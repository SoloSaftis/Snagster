//! Snagster's window: menus, capture flow, tools, the canvas and the tray of recent pictures.
//! The extra windows (settings, library, GIF editor, timer...) live in `windows.rs`.

use crate::capture::{self, CaptureKind, CaptureResult, CutShape, GifDone, MonitorInfo, PickPurpose, Shot, StepEvent, WindowInfo};
use crate::doc::{self, Doc, Shape, Tool};
use crate::effects::{self, Effect, EffectOptions};
use crate::hotkeys::{self, Hotkeys, Target};
use crate::overlay::{AreaAction, Outcome, Overlay, OverlayMode};
use crate::settings::{Action, PresetMode, Settings};
use crate::{export, fonts, library, ocr, render, sample};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Key, Modifiers, PointerButton, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, TextureHandle, TextureId, TextureOptions, Vec2, pos2, vec2,
};
use image::RgbaImage;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ACCENT: Color32 = Color32::from_rgb(79, 124, 255);
pub const BG_DARK: Color32 = Color32::from_rgb(24, 25, 30);
pub const PANEL: Color32 = Color32::from_rgb(33, 35, 42);
pub const PANEL_HI: Color32 = Color32::from_rgb(46, 49, 59);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 155, 170);

const UNDO_LIMIT: usize = 100;
const SCREENSHOT_FRAMES: u32 = 120;
/// The key used in the stamp texture cache for the mouse-pointer stamp.
pub const POINTER_KEY: char = '\u{E000}';
/// Start as the capture panel (the `--panel` option).
pub static START_PANEL: AtomicBool = AtomicBool::new(false);
/// Run the button test (the `--ui-test` option).
pub static UI_TEST: AtomicBool = AtomicBool::new(false);

mod uitest;
/// The editor window's smallest size (lifted while it is the panel or the recording bar).
pub const MIN_SIZE: Vec2 = Vec2::new(900.0, 560.0);

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub(crate) struct Entry {
    pub name: String,
    pub doc: Doc,
    pub undo: Vec<Doc>,
    pub redo: Vec<Doc>,
    tex: Option<(Arc<RgbaImage>, TextureHandle)>,
    pub path: Option<PathBuf>,
}

impl Entry {
    pub fn new(name: String, doc: Doc) -> Self {
        Self { name, doc, undo: Vec::new(), redo: Vec::new(), tex: None, path: None }
    }

    pub fn push_undo(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn texture(&mut self, ctx: &egui::Context) -> TextureId {
        let fresh = match &self.tex {
            Some((img, _)) => !Arc::ptr_eq(img, &self.doc.image),
            None => true,
        };
        if fresh {
            let img = self.doc.image.clone();
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            let handle = ctx.load_texture(format!("pic-{}", self.name), ci, TextureOptions::LINEAR);
            self.tex = Some((img, handle));
        }
        self.tex.as_ref().unwrap().1.id()
    }
}

enum Drag {
    None,
    Draw { start: Pos2, cur: Pos2, points: Vec<Pos2> },
    Move { idx: usize, last: Pos2, pushed: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Text,
    Callout { tail: Pos2 },
}

struct TextEditing {
    kind: TextKind,
    pos: Pos2,
    text: String,
    index: Option<usize>,
    color: Color32,
    size: f32,
    bg: bool,
    shadow: bool,
    font: String,
    bold: bool,
    italic: bool,
    focused: bool,
}

struct ScreenshotJob {
    path: PathBuf,
    frames: u32,
    requested: bool,
    result: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
}

pub(crate) struct Pin {
    pub id: u64,
    pub title: String,
    pub image: RgbaImage,
    pub tex: Option<TextureHandle>,
    pub close: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    AfterCapture,
    Saving,
    Capture,
    Recording,
    Editor,
    Effects,
    Redact,
    Hotkeys,
    Presets,
    Ai,
}

/// The small always-on-top bar shown while something runs in the background.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mini {
    Gif,
    Steps,
    Scroll,
}

pub(crate) enum OcrPurpose {
    GrabText,
    Redact(usize),
    Simplify(usize),
}

pub(crate) struct OcrResult {
    purpose: OcrPurpose,
    image: RgbaImage,
    lines: Result<Vec<ocr::Line>, String>,
}

pub(crate) struct IntervalJob {
    pub kind: CaptureKind,
    pub every: Duration,
    pub left: u32,
    pub next: Instant,
}

pub(crate) struct Recording {
    pub path: PathBuf,
    pub frames: u32,
    pub seconds: f32,
}

pub(crate) struct GifEditor {
    pub path: PathBuf,
    pub frames: Vec<(TextureHandle, u32)>,
    pub size: Vec2,
    pub index: usize,
    pub playing: bool,
    pub shown_at: Instant,
    pub trim: (usize, usize),
}

pub struct SnagApp {
    pub(crate) settings: Settings,
    saved_settings: Settings,
    pub(crate) persist: bool,
    pub(crate) entries: Vec<Entry>,
    pub(crate) current: Option<usize>,
    pub(crate) tool: Tool,
    zoom: Option<f32>,
    pan: Vec2,
    drag: Drag,
    selected: Option<usize>,
    text_edit: Option<TextEditing>,
    bold: bool,
    italic: bool,
    pub(crate) stamp: String,
    pub(crate) stamp_group: usize,
    pub(crate) font_query: String,
    // capture
    capturing: Option<CaptureKind>,
    hidden_for_capture: bool,
    quiet_capture: bool,
    cap_tx: Sender<CaptureResult>,
    cap_rx: Receiver<CaptureResult>,
    windows: Vec<WindowInfo>,
    windows_at: Option<Instant>,
    monitors: Vec<MonitorInfo>,
    monitors_at: Option<Instant>,
    overlay: Option<Overlay>,
    restore_pos: Option<Pos2>,
    pub(crate) last_region: Option<(i32, i32, u32, u32)>,
    pending_preset: Option<usize>,
    // background jobs
    scroll_rx: Option<Receiver<Result<RgbaImage, String>>>,
    pub(crate) job_stop: Option<Arc<AtomicBool>>,
    ocr_tx: Sender<OcrResult>,
    ocr_rx: Receiver<OcrResult>,
    pub(crate) ocr_busy: bool,
    pub(crate) grabbed_text: Option<String>,
    step_rx: Option<Receiver<StepEvent>>,
    pub(crate) guide: Vec<usize>,
    gif_tx: Sender<GifDone>,
    gif_rx: Receiver<GifDone>,
    pub(crate) recordings: Vec<Recording>,
    pub(crate) gif_editor: Option<GifEditor>,
    pub(crate) gif_open: bool,
    pub(crate) mini: Option<Mini>,
    mini_since: Instant,
    mini_restore: Option<(Pos2, Vec2)>,
    pub(crate) interval: Option<IntervalJob>,
    pub(crate) schedule: Option<(chrono::NaiveTime, CaptureKind)>,
    pub(crate) timer_open: bool,
    // library
    pub(crate) library_open: bool,
    pub(crate) lib_items: Vec<library::Item>,
    pub(crate) lib_loaded: bool,
    pub(crate) lib_query: String,
    pub(crate) lib_sort: usize,
    pub(crate) lib_app: String,
    pub(crate) run_at_startup: Option<bool>,
    pub(crate) lib_thumbs: HashMap<PathBuf, TextureHandle>,
    pub(crate) lib_thumb_rx: Option<Receiver<(PathBuf, RgbaImage)>>,
    pub(crate) lib_tag_edit: Option<(usize, String)>,
    // ui helpers
    ui_fonts: fonts::UiFonts,
    stamp_tex: HashMap<(char, u32), TextureHandle>,
    image_tex: HashMap<usize, (Arc<RgbaImage>, TextureHandle)>,
    pub(crate) pins: Vec<Pin>,
    pub(crate) next_pin: u64,
    pub(crate) hotkeys: Hotkeys,
    hk_rx: Receiver<u32>,
    hk_tx: Sender<u32>,
    hotkeys_for: Option<(bool, Vec<(Target, crate::settings::Combo)>)>,
    pub(crate) status: String,
    counter: u32,
    screenshot: Option<ScreenshotJob>,
    last_scale: f32,
    pub(crate) settings_open: bool,
    pub(crate) settings_tab: SettingsTab,
    pub(crate) shortcuts_open: bool,
    pub(crate) about_open: bool,
    pub(crate) redact_open: bool,
    pub(crate) guide_title: String,
    pub(crate) new_theme_name: String,
    tray: Option<crate::tray::Tray>,
    tray_tx: Sender<crate::tray::TrayCmd>,
    tray_rx: Receiver<crate::tray::TrayCmd>,
    tray_for: Option<(Vec<String>, Vec<String>)>,
    pub(crate) panel_open: bool,
    panel_restore: Option<(Pos2, Vec2)>,
    pub(crate) pending_panel: Option<(Option<Effect>, crate::settings::PanelShare)>,
    quitting: bool,
    pub(crate) mark_clip: Option<Shape>,
    pub(crate) dialog: Option<crate::windows::Dialog>,
    startup_done: bool,
    settings_changed_at: Option<Instant>,
    ui_test: Option<uitest::UiTest>,
    scroll_manual: bool,
}

impl SnagApp {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<PathBuf>, open: Option<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
        let check_run = screenshot.is_some() || UI_TEST.load(Ordering::Relaxed);
        let settings = Settings::load(check_run);
        let (cap_tx, cap_rx) = channel();
        let (hk_tx, hk_rx) = channel();
        let (ocr_tx, ocr_rx) = channel();
        let (gif_tx, gif_rx) = channel();
        let (tray_tx, tray_rx) = channel();
        fonts::start_scan(cc.egui_ctx.clone());
        let mut app = Self {
            saved_settings: settings.clone(),
            settings,
            persist: !check_run,
            entries: Vec::new(),
            current: None,
            tool: Tool::Arrow,
            zoom: None,
            pan: Vec2::ZERO,
            drag: Drag::None,
            selected: None,
            text_edit: None,
            bold: false,
            italic: false,
            stamp: "\u{2705}".into(),
            stamp_group: 0,
            font_query: String::new(),
            capturing: None,
            hidden_for_capture: false,
            quiet_capture: false,
            cap_tx,
            cap_rx,
            windows: Vec::new(),
            windows_at: None,
            monitors: Vec::new(),
            monitors_at: None,
            overlay: None,
            restore_pos: None,
            last_region: None,
            pending_preset: None,
            scroll_rx: None,
            job_stop: None,
            ocr_tx,
            ocr_rx,
            ocr_busy: false,
            grabbed_text: None,
            step_rx: None,
            guide: Vec::new(),
            gif_tx,
            gif_rx,
            recordings: Vec::new(),
            gif_editor: None,
            gif_open: false,
            mini: None,
            mini_since: Instant::now(),
            mini_restore: None,
            interval: None,
            schedule: None,
            timer_open: false,
            library_open: false,
            lib_items: Vec::new(),
            lib_loaded: false,
            lib_query: String::new(),
            lib_sort: 0,
            lib_app: String::new(),
            run_at_startup: None,
            lib_thumbs: HashMap::new(),
            lib_thumb_rx: None,
            lib_tag_edit: None,
            ui_fonts: fonts::UiFonts::default(),
            stamp_tex: HashMap::new(),
            image_tex: HashMap::new(),
            pins: Vec::new(),
            next_pin: 1,
            hotkeys: Hotkeys::none(""),
            hk_rx,
            hk_tx,
            hotkeys_for: None,
            status: "Ready. Press Print Screen, use the Capture menu, or paste a picture with Ctrl+V.".into(),
            counter: 0,
            screenshot: screenshot.map(|path| ScreenshotJob {
                path,
                frames: 0,
                requested: false,
                result: Arc::new(Mutex::new(None)),
            }),
            last_scale: 1.0,
            settings_open: false,
            settings_tab: SettingsTab::AfterCapture,
            shortcuts_open: false,
            about_open: false,
            redact_open: false,
            guide_title: "How to do it".into(),
            new_theme_name: String::new(),
            tray: None,
            tray_tx,
            tray_rx,
            tray_for: None,
            panel_open: false,
            panel_restore: None,
            pending_panel: None,
            quitting: false,
            mark_clip: None,
            dialog: None,
            startup_done: false,
            settings_changed_at: None,
            ui_test: None,
            scroll_manual: false,
        };
        if check_run {
            let mut second = Doc::new(sample::sample_image(1));
            second.shapes.push(Shape::Ellipse {
                rect: Rect::from_min_max(pos2(700.0, 210.0), pos2(1150.0, 390.0)),
                color: rgb(app.settings.palette[4]),
                width: 6.0,
                fill: false,
                shadow: false,
            });
            app.add_entry("Sample 2".into(), second);
            app.add_entry("Sample 1".into(), sample::sample_doc());
            if UI_TEST.load(Ordering::Relaxed) {
                let dir = std::env::temp_dir().join("snagster-ui-test");
                let _ = std::fs::remove_dir_all(&dir);
                let _ = std::fs::create_dir_all(&dir);
                app.settings.library_folder = dir.join("Library").display().to_string();
                app.settings.save_folder = dir.display().to_string();
                app.ui_test = Some(uitest::UiTest::new(dir));
            }
            if START_PANEL.load(Ordering::Relaxed) {
                app.panel_open = true;
                app.settings.panel.delay_on = true;
                app.status = String::new();
            }
            app.status = "Sample picture loaded. Pick a tool on the left and draw on it.".into();
        } else {
            app.apply_hotkeys(&cc.egui_ctx);
            app.apply_tray(&cc.egui_ctx);
        }
        if let Some(p) = open {
            app.open_path(&p);
        }
        app
    }

    pub(crate) fn color(&self) -> Color32 {
        rgb(self.settings.color)
    }

    pub(crate) fn add_entry(&mut self, name: String, d: Doc) {
        self.entries.push(Entry::new(name, d));
        self.current = Some(self.entries.len() - 1);
        self.zoom = None;
        self.pan = Vec2::ZERO;
        self.selected = None;
        self.text_edit = None;
        self.drag = Drag::None;
    }

    pub(crate) fn next_name(&mut self, what: &str) -> String {
        self.counter += 1;
        format!("{what} {}", self.counter)
    }

    pub(crate) fn entry(&mut self) -> Option<&mut Entry> {
        let i = self.current?;
        self.entries.get_mut(i)
    }

    // ---------- hotkeys ----------

    /// Register the hotkeys the settings ask for (again, when they changed).
    pub(crate) fn apply_hotkeys(&mut self, ctx: &egui::Context) {
        let want = (self.settings.hotkeys_enabled, hotkeys::wanted(&self.settings));
        if self.hotkeys_for.as_ref() == Some(&want) {
            return;
        }
        // Drop the old registrations first so the same keys can be taken again.
        self.hotkeys = Hotkeys::none("");
        let ctx2 = ctx.clone();
        self.hotkeys = hotkeys::register(&self.settings, self.hk_tx.clone(), move || ctx2.request_repaint());
        self.hotkeys_for = Some(want);
    }

    fn poll_hotkeys(&mut self, ctx: &egui::Context) {
        while let Ok(id) = self.hk_rx.try_recv() {
            match self.hotkeys.target(id) {
                Some(Target::Action(a)) => self.do_action(ctx, a),
                Some(Target::Preset(i)) => self.run_preset(ctx, i),
                None => {}
            }
        }
    }

    pub(crate) fn do_action(&mut self, ctx: &egui::Context, a: Action) {
        let pick = |s, p| CaptureKind::Pick(s, p);
        match a {
            Action::AllInOne => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::AllInOne)),
            Action::Area => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::Capture)),
            Action::Window => self.start_capture(ctx, CaptureKind::WindowPick),
            Action::Screen => self.start_capture(ctx, CaptureKind::Screen),
            Action::ActiveWindow => self.start_capture(ctx, CaptureKind::ActiveWindow),
            Action::AllScreens => self.start_capture(ctx, CaptureKind::AllScreens),
            Action::LastArea => match self.last_region {
                Some((x, y, w, h)) => self.start_capture(ctx, CaptureKind::LastRegion(x, y, w, h)),
                None => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::Capture)),
            },
            Action::Scrolling => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::Scrolling { horizontal: false })),
            Action::Panorama => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::Scrolling { horizontal: true })),
            Action::Circle => self.start_capture(ctx, pick(CutShape::Ellipse, PickPurpose::Capture)),
            Action::Freehand => self.start_capture(ctx, pick(CutShape::Freehand, PickPurpose::Capture)),
            Action::Polygon => self.start_capture(ctx, pick(CutShape::Polygon, PickPurpose::Capture)),
            Action::MultiArea => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::Multi)),
            Action::ManualScroll => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::ManualScroll)),
            Action::MenuCapture => {
                let d = self.settings.menu_delay_s.max(1);
                self.status = format!("Open the menu now: Snagster captures it in {d} seconds.");
                self.start_capture_with(ctx, CaptureKind::Menus, d);
            }
            Action::GrabText => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::GrabText)),
            Action::ColorPicker => self.start_capture(ctx, CaptureKind::ColorPicker),
            Action::Ruler => self.start_capture(ctx, CaptureKind::Ruler),
            Action::RecordGif => {
                if self.mini == Some(Mini::Gif) {
                    self.stop_job();
                } else {
                    self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::RecordGif));
                }
            }
            Action::RecordSteps => {
                if self.mini == Some(Mini::Steps) {
                    self.stop_steps(ctx);
                } else {
                    self.start_steps(ctx);
                }
            }
            Action::ShowSnag => self.show_window(ctx),
        }
    }

    pub(crate) fn run_preset(&mut self, ctx: &egui::Context, i: usize) {
        let Some(p) = self.settings.presets.get(i).cloned() else { return };
        let pick = |s| CaptureKind::Pick(s, PickPurpose::Capture);
        let kind = match p.mode {
            PresetMode::Area => pick(CutShape::Rect),
            PresetMode::Window => CaptureKind::WindowPick,
            PresetMode::Screen => CaptureKind::Screen,
            PresetMode::AllScreens => CaptureKind::AllScreens,
            PresetMode::FixedSize => CaptureKind::Fixed(p.width.max(8), p.height.max(8)),
            PresetMode::Circle => pick(CutShape::Ellipse),
            PresetMode::Freehand => pick(CutShape::Freehand),
            PresetMode::Star => pick(CutShape::Star),
            PresetMode::Heart => pick(CutShape::Heart),
            PresetMode::Scrolling => CaptureKind::Pick(CutShape::Rect, PickPurpose::Scrolling { horizontal: false }),
        };
        self.pending_preset = Some(i);
        self.start_capture_with(ctx, kind, p.delay_s);
    }

    // ---------- capturing ----------

    pub(crate) fn start_capture(&mut self, ctx: &egui::Context, kind: CaptureKind) {
        let d = self.settings.delay_s;
        self.start_capture_with(ctx, kind, d);
    }

    fn start_capture_with(&mut self, ctx: &egui::Context, kind: CaptureKind, delay_s: u64) {
        if self.capturing.is_some() || self.overlay.is_some() || self.mini.is_some() {
            return;
        }
        self.commit_text();
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
        let outer = ctx.input(|i| i.viewport().outer_rect);
        self.restore_pos = outer.map(|r| pos2(r.min.x * ppp, r.min.y * ppp));
        let at = outer.map(|r| ((r.center().x * ppp) as i32, (r.center().y * ppp) as i32)).unwrap_or((0, 0));
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        self.hidden_for_capture = minimized;
        let mut delay = 60;
        if self.settings.hide_window && !minimized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.hidden_for_capture = true;
            delay = self.settings.hide_settle_ms;
        }
        delay += delay_s * 1000;
        capture::spawn(kind.clone(), delay, at, ctx.clone(), self.cap_tx.clone());
        self.status = if delay_s > 0 { format!("Capturing in {delay_s} seconds...") } else { "Capturing...".into() };
        self.capturing = Some(kind);
    }

    pub(crate) fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn poll_capture(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.cap_rx.try_recv() {
            self.capturing = None;
            match res.shot {
                Ok(shot) if res.kind.uses_overlay() => {
                    let (mode, shape) = match &res.kind {
                        CaptureKind::Pick(s, p) => (OverlayMode::Pick(*p), *s),
                        CaptureKind::WindowPick => (OverlayMode::Window, CutShape::Rect),
                        CaptureKind::Fixed(w, h) => (OverlayMode::Fixed(*w, *h), CutShape::Rect),
                        CaptureKind::ColorPicker => (OverlayMode::ColorPicker, CutShape::Rect),
                        _ => (OverlayMode::Ruler, CutShape::Rect),
                    };
                    let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
                    let at = pos2(shot.origin.0 as f32 / ppp, shot.origin.1 as f32 / ppp);
                    self.overlay = Some(Overlay::new(mode, shape, shot));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(at));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Ok(shot) => {
                    let show = !self.quiet_capture && (!self.hidden_for_capture || self.settings.after_show_editor);
                    if show {
                        self.show_window(ctx);
                    } else if self.tray.is_some() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                    let title = match &res.kind {
                        CaptureKind::Window(_, t) => t.clone(),
                        CaptureKind::AllScreens => "All screens".into(),
                        CaptureKind::LastRegion(..) => "Area".into(),
                        _ => shot.title.clone(),
                    };
                    let cursor = shot.cursor.map(|c| pos2((c.0 - shot.origin.0) as f32, (c.1 - shot.origin.1) as f32));
                    let kind = match &res.kind {
                        CaptureKind::Screen | CaptureKind::Monitor(_) => "Screen",
                        CaptureKind::AllScreens => "All screens",
                        CaptureKind::Window(..) | CaptureKind::ActiveWindow => "Window",
                        _ => "Area",
                    };
                    self.finish_capture(shot.image, &title, &shot.app, kind, cursor);
                }
                Err(e) => {
                    self.show_window(ctx);
                    self.pending_preset = None;
                    self.status = format!("Capture failed: {e}");
                }
            }
        }
    }

    fn leave_overlay(&mut self, ctx: &egui::Context, show_editor: bool) {
        self.overlay = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        if let Some(p) = self.restore_pos {
            let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(p.x / ppp, p.y / ppp)));
        }
        if show_editor {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    /// A new picture arrived: put it in Recent and the library and run the after-capture steps
    /// (or the preset's).
    pub(crate) fn finish_capture(&mut self, mut img: RgbaImage, title: &str, app: &str, kind: &str, cursor: Option<Pos2>) {
        let preset = self.pending_preset.take().and_then(|i| self.settings.presets.get(i).cloned());
        let panel_job = self.pending_panel.take();
        let mut opts = self.effect_options();
        opts.info_text = format!("{}   {}   {}", chrono::Local::now().format("%Y-%m-%d %H:%M"), app, title);
        if let Some((Some(e), _)) = &panel_job {
            img = effects::apply(*e, &img, &opts);
        }
        if let Some(p) = &preset {
            for e in &p.effects {
                img = effects::apply(*e, &img, &opts);
            }
        }
        let (w, h) = img.dimensions();
        let short: String = title.chars().take(24).collect();
        let name = self.next_name(if short.trim().is_empty() { kind } else { short.trim() });
        let mut d = Doc::new(img.clone());
        if self.settings.include_cursor
            && let Some(c) = cursor
            && c.x >= 0.0
            && c.y >= 0.0
            && c.x < w as f32
            && c.y < h as f32
        {
            d.shapes.push(Shape::Pointer { pos: c, size: doc::pointer_size(4.0) });
        }
        self.add_entry(name, d);
        let mut notes = vec![format!("Captured {w} x {h}.")];
        if self.settings.after_library {
            let meta = library::Meta { title: title.to_string(), app: app.to_string(), kind: kind.into(), ..Default::default() };
            match library::add(std::path::Path::new(&self.settings.library_folder), &img, meta) {
                Ok(_) => self.lib_loaded = false,
                Err(e) => notes.push(format!("Not added to the library: {e}")),
            }
        }
        let (copy, save, pin) = match &preset {
            Some(p) => (p.copy, p.save, p.pin),
            None => (self.settings.after_copy, self.settings.after_save, self.settings.after_pin),
        };
        if copy {
            self.copy();
            notes.push("Copied.".into());
        }
        if save {
            self.quick_save();
            notes.push(self.status.clone());
        }
        if pin {
            self.pin_current();
        }
        if let Some((_, share)) = panel_job {
            self.panel_share(share);
        }
        if let Some(p) = preset {
            notes.push(format!("Preset \"{}\".", p.name));
        }
        self.status = notes.join(" ");
    }

    fn handle_overlay(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let settings = self.settings.clone();
        let Some(ov) = &mut self.overlay else { return };
        let outcome = ov.ui(ui, &settings);
        match outcome {
            Outcome::Pending => {}
            Outcome::Cancel => {
                self.pending_preset = None;
                self.status = "Capture cancelled.".into();
                self.leave_overlay(&ctx, true);
            }
            Outcome::Color(c) => {
                let hex = format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
                self.settings.color = c;
                let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(hex.clone()));
                self.status = format!("Picked {hex}. It's copied and set as the drawing color.");
                self.leave_overlay(&ctx, true);
            }
            Outcome::Multi(rects) => {
                let Some(ov) = self.overlay.take() else { return };
                let bb = rects.iter().copied().reduce(|a, b| a.union(b)).unwrap_or(Rect::NOTHING);
                let (bw, bh) = (bb.width().round().max(1.0) as u32, bb.height().round().max(1.0) as u32);
                let mut out = RgbaImage::new(bw, bh);
                for r in &rects {
                    let (x, y) = (r.min.x.round().max(0.0) as u32, r.min.y.round().max(0.0) as u32);
                    let w = (r.width().round() as u32).min(ov.shot.image.width().saturating_sub(x));
                    let h = (r.height().round() as u32).min(ov.shot.image.height().saturating_sub(y));
                    let part = image::imageops::crop_imm(&ov.shot.image, x, y, w, h).to_image();
                    image::imageops::replace(&mut out, &part, (r.min.x - bb.min.x).round() as i64, (r.min.y - bb.min.y).round() as i64);
                }
                let show = self.settings.after_show_editor || !self.hidden_for_capture;
                self.leave_overlay(&ctx, show);
                let n = rects.len();
                self.finish_capture(out, &format!("{n} areas"), &ov.shot.app, "Areas", None);
            }
            Outcome::FullScreen => {
                let Some(ov) = self.overlay.take() else { return };
                let show = self.settings.after_show_editor || !self.hidden_for_capture;
                self.leave_overlay(&ctx, show);
                let cursor = ov.shot.cursor.map(|c| pos2((c.0 - ov.shot.origin.0) as f32, (c.1 - ov.shot.origin.1) as f32));
                self.finish_capture(ov.shot.image, &ov.shot.title, &ov.shot.app, "Screen", cursor);
            }
            Outcome::Area { rect, shape, points, action } => {
                let Some(ov) = self.overlay.take() else { return };
                let x = rect.min.x.round().max(0.0) as u32;
                let y = rect.min.y.round().max(0.0) as u32;
                let w = (rect.width().round() as u32).min(ov.shot.image.width().saturating_sub(x)).max(1);
                let h = (rect.height().round() as u32).min(ov.shot.image.height().saturating_sub(y)).max(1);
                let mut img = image::imageops::crop_imm(&ov.shot.image, x, y, w, h).to_image();
                if shape != CutShape::Rect {
                    img = render::cut_to_shape(&img, shape, &points);
                }
                let desktop = (ov.shot.origin.0 + x as i32, ov.shot.origin.1 + y as i32, w, h);
                self.last_region = Some(desktop);
                let cursor = ov.shot.cursor.map(|c| pos2((c.0 - desktop.0) as f32, (c.1 - desktop.1) as f32));
                let show = self.settings.after_show_editor || !self.hidden_for_capture;
                match action {
                    AreaAction::Capture => {
                        self.leave_overlay(&ctx, show);
                        let title = if ov.mode == OverlayMode::Window { "Window" } else { shape.label() };
                        self.finish_capture(img, title, &ov.shot.app, "Area", cursor);
                    }
                    AreaAction::Copy => {
                        self.leave_overlay(&ctx, show);
                        self.finish_capture(img, "Area", &ov.shot.app, "Area", cursor);
                        self.copy();
                    }
                    AreaAction::GrabText => {
                        self.leave_overlay(&ctx, true);
                        self.run_ocr(OcrPurpose::GrabText, img);
                    }
                    AreaAction::Scroll { horizontal } => {
                        self.leave_overlay(&ctx, true);
                        self.start_scroll(&ctx, desktop, horizontal, false);
                    }
                    AreaAction::ManualScroll => {
                        self.leave_overlay(&ctx, true);
                        self.start_scroll(&ctx, desktop, false, true);
                    }
                    AreaAction::RecordGif => {
                        self.leave_overlay(&ctx, true);
                        self.start_gif(&ctx, desktop);
                    }
                }
            }
        }
    }

    // ---------- background jobs: scrolling, GIF, steps, text ----------

    fn enter_mini(&mut self, ctx: &egui::Context, kind: Mini) {
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        if let (Some(o), Some(i)) = (outer, inner) {
            self.mini_restore = Some((pos2(o.min.x * ppp, o.min.y * ppp), i.size()));
        }
        let (bw, bh) = (420.0, 56.0);
        let at = capture::cursor_pos().unwrap_or((0, 0));
        let mons = capture::monitors();
        let m = mons
            .iter()
            .find(|m| at.0 >= m.x && at.1 >= m.y && at.0 < m.x + m.width as i32 && at.1 < m.y + m.height as i32)
            .or(mons.first());
        if let Some(m) = m {
            let x = (m.x + m.width as i32) as f32 - (bw + 24.0) * ppp;
            let y = (m.y + m.height as i32) as f32 - (bh + 72.0) * ppp;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(200.0, 40.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(bw, bh)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(x / ppp, y / ppp)));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        }
        self.mini = Some(kind);
        self.mini_since = Instant::now();
    }

    fn leave_mini(&mut self, ctx: &egui::Context) {
        self.mini = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(MIN_SIZE));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        if let Some((pos, size)) = self.mini_restore.take() {
            let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(pos.x / ppp, pos.y / ppp)));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub(crate) fn stop_job(&mut self) {
        if let Some(s) = &self.job_stop {
            s.store(true, Ordering::Relaxed);
        }
    }

    fn start_scroll(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32), horizontal: bool, manual: bool) {
        self.scroll_manual = manual;
        let stop = Arc::new(AtomicBool::new(false));
        self.job_stop = Some(stop.clone());
        let (tx, rx) = channel();
        self.scroll_rx = Some(rx);
        let (steps, wait) = (self.settings.scroll_max_steps, self.settings.scroll_wait_ms);
        let ctx2 = ctx.clone();
        self.enter_mini(ctx, Mini::Scroll);
        std::thread::spawn(move || {
            // Let Snagster's bar settle before the first frame.
            std::thread::sleep(Duration::from_millis(400));
            let r = capture::scroll_capture(rect, horizontal, steps, wait, stop, manual);
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        self.status = if manual { "Scroll the page yourself; press Stop at the end.".into() } else { "Scrolling and stitching... keep the mouse still.".into() };
    }

    pub(crate) fn start_gif(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32)) {
        let stop = Arc::new(AtomicBool::new(false));
        self.job_stop = Some(stop.clone());
        let folder = PathBuf::from(&self.settings.library_folder).join("Recordings");
        let name = chrono::Local::now().format("Recording_%Y-%m-%d_%H-%M-%S").to_string();
        let path = crate::settings::unique_path(&folder, &name, "gif");
        self.enter_mini(ctx, Mini::Gif);
        capture::spawn_gif_recorder(
            rect,
            self.settings.gif_fps,
            self.settings.gif_max_s,
            self.settings.gif_max_width,
            self.settings.gif_cursor,
            path,
            stop,
            ctx.clone(),
            self.gif_tx.clone(),
        );
        self.status = "Recording a GIF...".into();
    }

    pub(crate) fn start_steps(&mut self, ctx: &egui::Context) {
        if self.mini.is_some() || self.overlay.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.job_stop = Some(stop.clone());
        let (tx, rx) = channel();
        self.step_rx = Some(rx);
        self.guide.clear();
        self.enter_mini(ctx, Mini::Steps);
        capture::spawn_step_recorder(stop, tx, ctx.clone());
        self.status = "Recording steps: every click takes a numbered picture.".into();
    }

    pub(crate) fn stop_steps(&mut self, ctx: &egui::Context) {
        self.stop_job();
        self.step_rx = None;
        self.leave_mini(ctx);
        self.status = format!("Recorded {} steps. Use File > Export guide to make a document.", self.guide.len());
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.scroll_rx
            && let Ok(r) = rx.try_recv()
        {
            self.scroll_rx = None;
            self.job_stop = None;
            self.leave_mini(ctx);
            match r {
                Ok(img) => self.finish_capture(img, "Scrolling capture", "", "Scrolling", None),
                Err(e) => self.status = format!("Scrolling capture failed: {e}"),
            }
        }
        while let Ok(done) = self.gif_rx.try_recv() {
            self.job_stop = None;
            if self.mini == Some(Mini::Gif) {
                self.leave_mini(ctx);
            }
            match done.result {
                Ok(()) => {
                    self.status = format!("Recorded {} frames ({:.1} s). Open Tools > GIF recordings to trim or grab frames.", done.frames, done.seconds);
                    self.recordings.insert(0, Recording { path: done.path.clone(), frames: done.frames, seconds: done.seconds });
                    self.open_gif(ctx, done.path);
                }
                Err(e) => self.status = format!("Recording failed: {e}"),
            }
        }
        let mut steps = Vec::new();
        if let Some(rx) = &self.step_rx {
            while let Ok(ev) = rx.try_recv() {
                steps.push(ev);
            }
        }
        for ev in steps {
            let n = self.guide.len() as u32 + 1;
            let mut d = Doc::new(ev.image);
            let c = pos2(ev.click.0, ev.click.1);
            d.shapes.push(Shape::Pointer { pos: c, size: doc::pointer_size(5.0) });
            d.shapes.push(Shape::Step {
                center: c + vec2(-26.0, -26.0),
                n,
                color: self.color(),
                radius: doc::step_radius(6.0),
                shadow: true,
                style: self.settings.step_style,
            });
            let title = if ev.title.is_empty() { "Click".to_string() } else { ev.title.clone() };
            let caption: String = format!("Step {n}: {title}").chars().take(60).collect();
            self.entries.push(Entry::new(caption, d));
            self.guide.push(self.entries.len() - 1);
            if self.settings.after_library {
                let _ = library::add(
                    std::path::Path::new(&self.settings.library_folder),
                    &self.entries.last().map(|e| render::compose(&e.doc)).unwrap_or_default(),
                    library::Meta { title, app: ev.app, kind: "Step".into(), ..Default::default() },
                );
                self.lib_loaded = false;
            }
        }
        while let Ok(r) = self.ocr_rx.try_recv() {
            self.ocr_busy = false;
            self.ocr_done(r);
        }
    }

    pub(crate) fn run_ocr(&mut self, purpose: OcrPurpose, image: RgbaImage) {
        self.ocr_busy = true;
        self.status = "Reading text...".into();
        let tx = self.ocr_tx.clone();
        std::thread::spawn(move || {
            let lines = ocr::recognize(&image);
            let _ = tx.send(OcrResult { purpose, image, lines });
        });
    }

    fn ocr_done(&mut self, r: OcrResult) {
        let lines = match r.lines {
            Ok(l) => l,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        match r.purpose {
            OcrPurpose::GrabText => {
                let text = ocr::plain_text(&lines);
                if text.trim().is_empty() {
                    self.status = "No text was found there.".into();
                } else {
                    let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone()));
                    self.status = format!("Read {} lines of text; they're copied to the clipboard.", lines.len());
                    self.grabbed_text = Some(text);
                }
            }
            OcrPurpose::Redact(i) => {
                let s = &self.settings;
                let kinds = ocr::RedactKinds {
                    emails: s.redact_emails,
                    phones: s.redact_phones,
                    cards: s.redact_cards,
                    addresses: s.redact_addresses,
                    ips: s.redact_ips,
                    urls: s.redact_urls,
                };
                let found = ocr::find_sensitive(&lines, kinds);
                let block = self.settings.blur_block;
                if let Some(e) = self.entries.get_mut(i) {
                    if found.is_empty() {
                        self.status = "Nothing sensitive was found.".into();
                    } else {
                        e.push_undo();
                        let mut d = e.doc.clone();
                        for (r, _) in &found {
                            if let Some(nd) = d.blur(*r, block) {
                                d = nd;
                            }
                        }
                        e.doc = d;
                        let mut kinds: Vec<&str> = found.iter().map(|f| f.1).collect();
                        kinds.dedup();
                        self.status = format!("Blurred {} items ({}). Undo brings them back.", found.len(), kinds.join(", "));
                    }
                }
            }
            OcrPurpose::Simplify(i) => {
                let out = ocr::simplify(&r.image, &lines);
                if let Some(e) = self.entries.get_mut(i) {
                    e.push_undo();
                    e.doc = Doc::new(out);
                    self.status = format!("Simplified: {} lines of text became bars. Undo brings them back.", lines.len());
                }
            }
        }
    }

    pub(crate) fn open_gif(&mut self, ctx: &egui::Context, path: PathBuf) {
        match export::gif_frames(&path, 480) {
            Ok(frames) if !frames.is_empty() => {
                let size = vec2(frames[0].0.width() as f32, frames[0].0.height() as f32);
                let n = frames.len();
                let texs = frames
                    .into_iter()
                    .enumerate()
                    .map(|(i, (img, ms))| {
                        let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                        (ctx.load_texture(format!("gif-{i}"), ci, TextureOptions::LINEAR), ms)
                    })
                    .collect();
                self.gif_editor = Some(GifEditor {
                    path,
                    frames: texs,
                    size,
                    index: 0,
                    playing: true,
                    shown_at: Instant::now(),
                    trim: (0, n.saturating_sub(1)),
                });
                self.gif_open = true;
            }
            Ok(_) => self.status = "That GIF has no frames.".into(),
            Err(e) => self.status = format!("Couldn't open the GIF: {e}"),
        }
    }

    // ---------- timers ----------

    fn poll_timers(&mut self, ctx: &egui::Context) {
        if let Some(job) = &mut self.interval
            && Instant::now() >= job.next
            && self.capturing.is_none()
        {
            let kind = job.kind.clone();
            job.left = job.left.saturating_sub(1);
            job.next = Instant::now() + job.every;
            let done = job.left == 0;
            self.quiet_capture = true;
            self.start_capture_with(ctx, kind, 0);
            if done {
                self.interval = None;
                self.quiet_capture = false;
                self.status = "Interval captures finished.".into();
            }
        }
        if let Some((at, kind)) = &self.schedule {
            let now = chrono::Local::now().time();
            if now >= *at && (now - *at).num_seconds() < 60 {
                let kind = kind.clone();
                self.schedule = None;
                self.start_capture_with(ctx, kind, 0);
            }
        }
        if self.interval.is_some() || self.schedule.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        } else if self.capturing.is_none() {
            self.quiet_capture = false;
        }
    }

    // ---------- files and clipboard ----------

    pub(crate) fn open_path(&mut self, p: &std::path::Path) {
        if p.is_file() {
            self.remember_file(p);
        }
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "snag" {
            match export::load_project(p) {
                Ok((name, d)) => {
                    self.add_entry(if name.is_empty() { "Project".into() } else { name }, d);
                    if let Some(e) = self.entry() {
                        e.path = Some(p.to_path_buf());
                    }
                    self.status = format!("Opened project {}", p.display());
                }
                Err(e) => self.status = format!("Couldn't open {}: {e}", p.display()),
            }
            return;
        }
        match image::open(p) {
            Ok(img) => {
                let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Picture".into());
                self.add_entry(name, Doc::new(img.to_rgba8()));
                if let Some(e) = self.entry() {
                    e.path = Some(p.to_path_buf());
                }
                self.status = format!("Opened {}", p.display());
            }
            Err(e) => self.status = format!("Couldn't open {}: {e}", p.display()),
        }
    }

    pub(crate) fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Pictures and Snagster projects", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "ico", "snag"])
            .pick_file()
        {
            self.open_path(&p);
        }
    }

    pub(crate) fn paste(&mut self) {
        let got = arboard::Clipboard::new().and_then(|mut c| c.get_image());
        match got {
            Ok(img) => {
                let w = img.width as u32;
                let h = img.height as u32;
                if let Some(rgba) = RgbaImage::from_raw(w, h, img.bytes.into_owned()) {
                    let name = self.next_name("Pasted");
                    self.add_entry(name, Doc::new(rgba));
                    self.status = format!("Pasted a {w} x {h} picture.");
                }
            }
            Err(_) => self.status = "There's no picture on the clipboard.".into(),
        }
    }

    pub(crate) fn copy(&mut self) {
        self.commit_text();
        let Some(e) = self.entry() else { return };
        let out = render::compose(&e.doc);
        let (w, h) = out.dimensions();
        let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: out.into_raw().into() };
        self.status = match arboard::Clipboard::new().and_then(|mut c| c.set_image(data)) {
            Ok(()) => format!("Copied the {w} x {h} picture. Paste it anywhere."),
            Err(e) => format!("Couldn't copy: {e}"),
        };
    }

    pub(crate) fn save_to(&mut self, i: usize, path: PathBuf) {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let result = if ext == "snag" {
            export::save_project(&self.entries[i].doc, &self.entries[i].name, &path)
        } else {
            export::save_image(&render::compose(&self.entries[i].doc), &path, self.settings.jpeg_quality)
        };
        self.status = match result {
            Ok(()) => {
                self.entries[i].path = Some(path.clone());
                self.remember_file(&path);
                format!("Saved {}", path.display())
            }
            Err(e) => format!("Couldn't save: {e}"),
        };
    }

    pub(crate) fn save_as(&mut self) {
        self.commit_text();
        let Some(i) = self.current else { return };
        let title = self.entries[i].name.clone();
        let name = self.settings.file_name(&title);
        let ext = self.settings.format.ext();
        let mut dialog = rfd::FileDialog::new();
        let first = self.settings.format;
        dialog = dialog.add_filter(first.label(), &[first.ext()]);
        for f in crate::settings::ImageFormat::ALL {
            if f != first {
                dialog = dialog.add_filter(f.label(), &[f.ext()]);
            }
        }
        dialog = dialog.add_filter("Snagster project (keeps marks editable)", &["snag"]);
        let folder = PathBuf::from(&self.settings.save_folder);
        if folder.is_dir() {
            dialog = dialog.set_directory(&folder);
        }
        if let Some(path) = dialog.set_file_name(format!("{name}.{ext}")).save_file() {
            self.save_to(i, path);
        }
    }

    pub(crate) fn save_project_as(&mut self) {
        self.commit_text();
        let Some(i) = self.current else { return };
        let title = self.entries[i].name.clone();
        let name = self.settings.file_name(&title);
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Snagster project", &["snag"])
            .set_file_name(format!("{name}.snag"))
            .save_file()
        {
            self.save_to(i, path);
        }
    }

    /// Save straight into the save folder with the file-name pattern.
    pub(crate) fn quick_save(&mut self) {
        self.commit_text();
        let Some(i) = self.current else { return };
        let title = self.entries[i].name.clone();
        let path = self.settings.next_save_path(&title);
        self.save_to(i, path);
    }

    pub(crate) fn open_folder(&mut self, target: &std::path::Path) {
        if target.is_dir() || target.extension().is_none() {
            let _ = std::fs::create_dir_all(target);
        }
        #[cfg(windows)]
        let r = if target.is_file() {
            std::process::Command::new("explorer").arg(format!("/select,{}", target.display())).spawn()
        } else {
            std::process::Command::new("explorer").arg(target).spawn()
        };
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg(target).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let r = std::process::Command::new("xdg-open").arg(target).spawn();
        if let Err(e) = r {
            self.status = format!("Couldn't open the folder: {e}");
        }
    }

    /// Open the picture in the computer's default program for its file type.
    pub(crate) fn open_externally(&mut self) {
        self.commit_text();
        let Some(i) = self.current else { return };
        let path = std::env::temp_dir().join(format!("snag-share-{}.png", chrono::Local::now().format("%H%M%S")));
        if let Err(e) = export::save_image(&render::compose(&self.entries[i].doc), &path, 90) {
            self.status = format!("Couldn't prepare the file: {e}");
            return;
        }
        #[cfg(windows)]
        let r = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(&path).spawn();
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg(&path).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let r = std::process::Command::new("xdg-open").arg(&path).spawn();
        if let Err(e) = r {
            self.status = format!("Couldn't open it: {e}");
        }
    }

    /// The pictures for a guide: the recorded steps, or else everything in Recent.
    pub(crate) fn guide_items(&self) -> Vec<(String, RgbaImage)> {
        let idx: Vec<usize> = if self.guide.iter().all(|i| *i < self.entries.len()) && !self.guide.is_empty() {
            self.guide.clone()
        } else {
            (0..self.entries.len()).collect()
        };
        idx.into_iter().map(|i| (self.entries[i].name.clone(), render::compose(&self.entries[i].doc))).collect()
    }

    pub(crate) fn export_guide(&mut self, format: &str) {
        self.commit_text();
        let items = self.guide_items();
        if items.is_empty() {
            self.status = "There's nothing to put in a guide yet.".into();
            return;
        }
        let title = self.guide_title.clone();
        if format == "page" {
            if let Some(img) = effects::guide_page(&title, &items, 1200) {
                let name = self.next_name("Guide");
                self.add_entry(name, Doc::new(img));
                self.status = format!("Made a one-page guide from {} pictures.", items.len());
            }
            return;
        }
        let ext = if format == "pdf" { "pdf" } else { "html" };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(if ext == "pdf" { "PDF document" } else { "Web page" }, &[ext])
            .set_file_name(format!("{title}.{ext}"))
            .save_file()
        else {
            return;
        };
        let result = if ext == "pdf" {
            let pages: Vec<(Option<String>, RgbaImage)> = items.into_iter().map(|(c, i)| (Some(c), i)).collect();
            export::pdf(&pages, true, self.settings.jpeg_quality).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()))
        } else {
            export::html_guide(&title, &items).and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string()))
        };
        self.status = match result {
            Ok(()) => format!("Saved the guide to {}", path.display()),
            Err(e) => format!("Couldn't save the guide: {e}"),
        };
    }

    // ---------- editing ----------

    pub(crate) fn undo(&mut self) {
        self.commit_text();
        if let Some(e) = self.entry()
            && let Some(prev) = e.undo.pop()
        {
            let cur = std::mem::replace(&mut e.doc, prev);
            e.redo.push(cur);
        }
        self.selected = None;
    }

    pub(crate) fn redo(&mut self) {
        if let Some(e) = self.entry()
            && let Some(next) = e.redo.pop()
        {
            let cur = std::mem::replace(&mut e.doc, next);
            e.undo.push(cur);
        }
        self.selected = None;
    }

    fn delete_selected(&mut self) {
        if let Some(s) = self.selected.take()
            && let Some(e) = self.entry()
            && s < e.doc.shapes.len()
        {
            e.push_undo();
            e.doc.shapes.remove(s);
        }
    }

    fn clear_marks(&mut self) {
        self.commit_text();
        self.selected = None;
        if let Some(e) = self.entry()
            && !e.doc.shapes.is_empty()
        {
            e.push_undo();
            e.doc.shapes.clear();
        }
    }

    fn scale_selected(&mut self, f: f32) {
        if let Some(s) = self.selected
            && let Some(e) = self.entry()
            && s < e.doc.shapes.len()
        {
            e.push_undo();
            e.doc.shapes[s].scale(f);
        }
    }

    fn set_color(&mut self, c: Color32) {
        self.settings.color = [c.r(), c.g(), c.b()];
        if let Some(t) = &mut self.text_edit {
            t.color = c;
        }
        if let Some(s) = self.selected
            && let Some(e) = self.entry()
            && s < e.doc.shapes.len()
        {
            e.push_undo();
            e.doc.shapes[s].set_color(c);
        }
    }

    pub(crate) fn effect_options(&self) -> EffectOptions {
        EffectOptions {
            resize_percent: self.settings.resize_percent,
            border_width: self.settings.border_width,
            border_color: self.settings.border_color,
            shadow_size: self.settings.shadow_size,
            rounded_radius: self.settings.rounded_radius,
            watermark_text: self.settings.watermark_text.clone(),
            tilt_percent: self.settings.tilt_percent,
            brightness: self.settings.brightness,
            contrast: self.settings.contrast,
            saturation: self.settings.saturation,
            info_text: {
                let name = self.current.map(|i| self.entries[i].name.clone()).unwrap_or_default();
                format!("{}   {}", chrono::Local::now().format("%Y-%m-%d %H:%M"), name)
            },
        }
    }

    fn apply_effect(&mut self, eff: Effect) {
        self.commit_text();
        self.selected = None;
        let opts = self.effect_options();
        let Some(e) = self.entry() else { return };
        let flat = render::compose(&e.doc);
        let out = effects::apply(eff, &flat, &opts);
        let (w, h) = out.dimensions();
        e.push_undo();
        e.doc = Doc::new(out);
        self.zoom = None;
        self.pan = Vec2::ZERO;
        self.status = format!("{} applied ({w} x {h}). Undo brings the marks back.", eff.label());
    }

    fn combine(&mut self, vertical: bool) {
        self.commit_text();
        if self.entries.len() < 2 {
            self.status = "Combining needs at least two pictures in Recent.".into();
            return;
        }
        let imgs: Vec<RgbaImage> = self.entries.iter().map(|e| render::compose(&e.doc)).collect();
        if let Some(out) = effects::combine(&imgs, vertical, 16) {
            let name = self.next_name("Combined");
            let n = imgs.len();
            self.add_entry(name, Doc::new(out));
            self.status = format!("Combined {n} pictures.");
        }
    }

    fn duplicate(&mut self) {
        let Some(i) = self.current else { return };
        let d = self.entries[i].doc.clone();
        let name = format!("{} copy", self.entries[i].name);
        self.add_entry(name, d);
    }

    fn remove_current(&mut self) {
        let Some(i) = self.current else { return };
        self.entries.remove(i);
        self.guide.retain(|g| *g != i);
        for g in &mut self.guide {
            if *g > i {
                *g -= 1;
            }
        }
        self.current = if self.entries.is_empty() { None } else { Some(i.min(self.entries.len() - 1)) };
        self.selected = None;
        self.text_edit = None;
    }

    pub(crate) fn pin_current(&mut self) {
        self.commit_text();
        let Some(i) = self.current else { return };
        let image = render::compose(&self.entries[i].doc);
        let title = self.entries[i].name.clone();
        self.pins.push(Pin { id: self.next_pin, title, image, tex: None, close: false });
        self.next_pin += 1;
        self.status = "Pinned on top of other windows. Drag to move it, double-click or Esc to close.".into();
    }

    fn commit_text(&mut self) {
        let Some(t) = self.text_edit.take() else { return };
        let (to, ta, cs, co) = (self.settings.text_outline, self.settings.text_align, self.settings.callout_shape, self.settings.callout_outline);
        let Some(e) = self.entry() else { return };
        let text = t.text.trim_end().to_string();
        match t.index {
            Some(i) if i < e.doc.shapes.len() => {
                e.push_undo();
                if text.is_empty() {
                    e.doc.shapes.remove(i);
                } else {
                    match &mut e.doc.shapes[i] {
                        Shape::Text { text: old, color, font, bold, italic, .. }
                        | Shape::Callout { text: old, color, font, bold, italic, .. } => {
                            *old = text;
                            *color = t.color;
                            *font = t.font;
                            *bold = t.bold;
                            *italic = t.italic;
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                if !text.is_empty() {
                    e.push_undo();
                    e.doc.shapes.push(match t.kind {
                        TextKind::Text => Shape::Text {
                            pos: t.pos,
                            text,
                            color: t.color,
                            size: t.size,
                            bg: t.bg,
                            shadow: t.shadow,
                            font: t.font,
                            bold: t.bold,
                            italic: t.italic,
                            outline: to,
                            align: ta,
                        },
                        TextKind::Callout { tail } => Shape::Callout {
                            pos: t.pos,
                            tail,
                            text,
                            color: t.color,
                            size: t.size,
                            shadow: t.shadow,
                            font: t.font,
                            bold: t.bold,
                            italic: t.italic,
                            shape: cs,
                            outline: co,
                            align: ta,
                        },
                    });
                }
            }
        }
    }

    fn finish_draw(&mut self, start: Pos2, cur: Pos2, points: Vec<Pos2>, shift: bool) {
        let (tool, color, width) = (self.tool, self.color(), self.settings.width);
        let (fill, radius, shadow, block) =
            (self.settings.fill, self.settings.corner_radius, self.settings.shadow, self.settings.blur_block);
        let Some(ci) = self.current else { return };
        let far = start.distance(cur) >= doc::MIN_DRAG;
        let shape = match tool {
            Tool::Arrow | Tool::Line if far => {
                let b = if shift { doc::snap45(start, cur) } else { cur };
                Some(if tool == Tool::Arrow {
                    Shape::Arrow { a: start, b, color, width, shadow, heads: self.settings.arrow_heads }
                } else {
                    Shape::Line { a: start, b, color, width, shadow }
                })
            }
            Tool::Rect | Tool::Ellipse if far => {
                let b = if shift { doc::square(start, cur) } else { cur };
                let rect = Rect::from_two_pos(start, b);
                Some(if tool == Tool::Rect {
                    Shape::Rect { rect, color, width, fill, radius, shadow }
                } else {
                    Shape::Ellipse { rect, color, width, fill, shadow }
                })
            }
            Tool::Spotlight if far => Some(Shape::Spotlight { rect: Rect::from_two_pos(start, cur) }),
            Tool::Magnify if far => Some(Shape::Magnify { center: start, radius: start.distance(cur), zoom: self.settings.magnify_zoom }),
            Tool::Pen if !points.is_empty() => Some(Shape::Pen { points, color, width, shadow }),
            Tool::Highlight if !points.is_empty() => {
                let points = if shift { vec![start, pos2(cur.x, start.y)] } else { points };
                Some(Shape::Highlight { points, color, width })
            }
            Tool::Callout => {
                // The press is the tip; the bubble goes where the drag ended.
                let size = doc::text_size(width);
                let pos = if far { cur } else { start + vec2(size * 2.0, -size * 3.0) };
                self.start_text_kind(ci, pos, None, TextKind::Callout { tail: start });
                None
            }
            Tool::Blur if far => {
                let mode = self.settings.blur_mode;
                let e = &mut self.entries[ci];
                if let Some(d) = e.doc.redact(Rect::from_two_pos(start, cur), mode, block, color) {
                    e.push_undo();
                    e.doc = d;
                }
                None
            }
            Tool::Eraser if !points.is_empty() => {
                let e = &mut self.entries[ci];
                if let Some(d) = e.doc.erase(&points, width * 3.0) {
                    e.push_undo();
                    e.doc = d;
                }
                None
            }
            Tool::Selection if far => {
                let e = &mut self.entries[ci];
                if let Some(d) = e.doc.lift(Rect::from_two_pos(start, cur)) {
                    e.push_undo();
                    let n = d.shapes.len();
                    e.doc = d;
                    self.tool = Tool::Select;
                    self.selected = Some(n - 1);
                    self.status = "Lifted out. Drag it to move it, [ and ] resize it, Delete removes it.".into();
                }
                None
            }
            Tool::CutOut if far => {
                let e = &mut self.entries[ci];
                if let Some(d) = e.doc.cut_out(Rect::from_two_pos(start, cur)) {
                    e.push_undo();
                    let (w, h) = d.image.dimensions();
                    e.doc = d;
                    self.status = format!("Cut out a band; the picture is now {w} x {h}.");
                }
                None
            }
            Tool::Crop if far => {
                let e = &mut self.entries[ci];
                if let Some(d) = e.doc.crop(Rect::from_two_pos(start, cur)) {
                    e.push_undo();
                    let (w, h) = d.image.dimensions();
                    e.doc = d;
                    self.zoom = None;
                    self.pan = Vec2::ZERO;
                    self.tool = Tool::Arrow;
                    self.status = format!("Cropped to {w} x {h}.");
                }
                None
            }
            _ => None,
        };
        if let Some(s) = shape {
            let e = &mut self.entries[ci];
            e.push_undo();
            e.doc.shapes.push(s);
        }
    }

    fn pick(&self, p: Pos2) -> Option<usize> {
        let e = &self.entries[self.current?];
        let tol = 6.0 / self.last_scale.max(0.01);
        e.doc.shapes.iter().rposition(|s| s.hit(p, tol))
    }

    fn set_zoom(&mut self, ctx: &egui::Context, z: Option<f32>) {
        match z {
            None => {
                self.zoom = None;
                self.pan = Vec2::ZERO;
            }
            Some(z) if z <= 0.0 => {
                self.zoom = Some(1.0 / ctx.pixels_per_point());
                self.pan = Vec2::ZERO;
            }
            Some(f) => self.zoom = Some((self.last_scale * f).clamp(0.02, 16.0)),
        }
    }

    // ---------- keyboard ----------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = self.text_edit.is_some() || ctx.memory(|m| m.focused().is_some());
        let consume = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || consume(Modifiers::COMMAND, Key::Y) {
            self.redo();
        }
        if consume(Modifiers::COMMAND, Key::Z) {
            self.undo();
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            self.quick_save();
        }
        if consume(Modifiers::COMMAND, Key::S) {
            self.save_as();
        }
        if consume(Modifiers::COMMAND, Key::O) {
            self.open_dialog();
        }
        if consume(Modifiers::COMMAND, Key::Comma) {
            self.settings_open = true;
        }
        if consume(Modifiers::COMMAND, Key::P) {
            self.pin_current();
        }
        if consume(Modifiers::COMMAND, Key::L) {
            self.library_open = true;
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::A) {
            self.do_action(ctx, Action::AllInOne);
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::W) {
            self.do_action(ctx, Action::Window);
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::F) {
            self.do_action(ctx, Action::Screen);
        }
        if consume(Modifiers::NONE, Key::F1) {
            self.shortcuts_open = true;
        }
        if typing {
            return;
        }
        let (copy_ev, paste_ev) = ctx.input(|i| {
            (
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
            )
        });
        let cut_ev = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Cut))) || consume(Modifiers::COMMAND, Key::X);
        if cut_ev {
            self.cut_mark();
        }
        if (copy_ev || consume(Modifiers::COMMAND, Key::C)) && self.current.is_some() {
            if self.selected.is_some() {
                self.copy_mark();
            } else {
                self.mark_clip = None;
                self.copy();
            }
        }
        if paste_ev || consume(Modifiers::COMMAND, Key::V) {
            if self.mark_clip.is_some() && self.current.is_some() {
                self.paste_mark();
            } else {
                self.paste();
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            self.delete_selected();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.selected = None;
            self.drag = Drag::None;
        }
        if ctx.input(|i| i.modifiers.any()) {
            return;
        }
        if self.selected.is_some() {
            if ctx.input(|i| i.key_pressed(Key::CloseBracket)) {
                self.scale_selected(1.15);
            }
            if ctx.input(|i| i.key_pressed(Key::OpenBracket)) {
                self.scale_selected(1.0 / 1.15);
            }
        }
        for t in Tool::ALL {
            if let Some(key) = Key::from_name(t.key())
                && ctx.input(|i| i.key_pressed(key))
            {
                self.tool = t;
                self.selected = None;
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Num0)) {
            self.set_zoom(ctx, None);
        }
        if ctx.input(|i| i.key_pressed(Key::Num1)) {
            self.set_zoom(ctx, Some(0.0));
        }
        if ctx.input(|i| i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals)) {
            self.set_zoom(ctx, Some(1.25));
        }
        if ctx.input(|i| i.key_pressed(Key::Minus)) {
            self.set_zoom(ctx, Some(0.8));
        }
    }

    // ---------- menus ----------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| self.file_menu(ui));
            ui.menu_button("Capture", |ui| self.capture_menu(ui, &ctx));
            ui.menu_button("Record", |ui| self.record_menu(ui, &ctx));
            ui.menu_button("Edit", |ui| self.edit_menu(ui));
            ui.menu_button("Image", |ui| self.image_menu(ui));
            ui.menu_button("Tools", |ui| self.tools_menu(ui, &ctx));
            ui.menu_button("Share", |ui| self.share_menu(ui));
            ui.menu_button("View", |ui| self.view_menu(ui, &ctx));
            ui.menu_button("Help", |ui| {
                if item(ui, "Getting started", "") {
                    self.dialog = Some(crate::windows::Dialog::GettingStarted);
                }
                if item(ui, "What's new", "") {
                    self.dialog = Some(crate::windows::Dialog::WhatsNew);
                }
                if item(ui, "Keyboard shortcuts", "F1") {
                    self.shortcuts_open = true;
                }
                if item(ui, "Hotkey settings...", "") {
                    self.settings_open = true;
                    self.settings_tab = SettingsTab::Hotkeys;
                }
                if item(ui, "About Snagster", "") {
                    self.about_open = true;
                }
            });
        });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let has = self.current.is_some();
        if item(ui, "New from clipboard", "Ctrl+V") {
            self.paste();
        }
        if item(ui, "New image...", "Ctrl+N") {
            self.dialog = Some(crate::windows::Dialog::new_image());
        }
        ui.menu_button("New from template", |ui| self.template_menu(ui));
        if item(ui, "Open picture or project...", "Ctrl+O") {
            self.open_dialog();
        }
        ui.menu_button("Open recent", |ui| {
            if self.settings.recent_files.is_empty() {
                ui.label(RichText::new("Nothing opened or saved yet").color(TEXT_DIM));
            }
            let mut open = None;
            for f in &self.settings.recent_files {
                let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
                if ui.button(name).on_hover_text(f).clicked() {
                    open = Some(PathBuf::from(f));
                    ui.close();
                }
            }
            if let Some(p) = open {
                self.open_path(&p);
            }
        });
        if item(ui, "Import pictures...", "") {
            if let Some(files) = rfd::FileDialog::new().add_filter("Pictures and Snagster projects", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "ico", "snag"]).pick_files() {
                for f in files {
                    self.open_path(&f);
                }
            }
        }
        if item(ui, "Library...", "Ctrl+L") {
            self.library_open = true;
        }
        ui.separator();
        if item_on(ui, has, "Save", "Ctrl+S") {
            self.save();
        }
        if item_on(ui, has, "Save as...", "Ctrl+Shift+Alt+S") {
            self.save_as();
        }
        if item_on(ui, has, "Rename...", "F2") {
            if let Some(i) = self.current {
                self.dialog = Some(crate::windows::Dialog::Rename(self.entries[i].name.clone()));
            }
        }
        if item_on(ui, has, "Move to Recycle Bin", "") {
            self.recycle_current();
        }
        if item_on(ui, !self.entries.is_empty(), "Export pictures...", "") {
            self.dialog = Some(crate::windows::Dialog::export(self.entries.len(), &self.settings));
        }
        if item_on(ui, has, "Print...", "Ctrl+P") {
            self.print();
        }
        if item_on(ui, has, "Save as Snagster project (editable marks)...", "") {
            self.save_project_as();
        }
        if item_on(ui, has, "Quick save to folder", "Ctrl+Shift+S") {
            self.quick_save();
        }
        ui.menu_button("Export guide", |ui| {
            ui.label(RichText::new("From recorded steps, or all of Recent").color(TEXT_DIM));
            ui.horizontal(|ui| {
                ui.label("Title");
                ui.text_edit_singleline(&mut self.guide_title);
            });
            if item(ui, "PDF document...", "") {
                self.export_guide("pdf");
            }
            if item(ui, "Web page...", "") {
                self.export_guide("html");
            }
            if item(ui, "One-page picture", "") {
                self.export_guide("page");
            }
        });
        ui.separator();
        let path = self.current.and_then(|i| self.entries[i].path.clone());
        if item_on(ui, path.is_some(), "Copy file path", "")
            && let Some(p) = &path
        {
            let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(p.display().to_string()));
            self.status = format!("Copied {}", p.display());
        }
        if item(ui, "Open save folder", "") {
            let f = PathBuf::from(&self.settings.save_folder);
            self.open_folder(&f);
        }
        ui.separator();
        if item(ui, "Settings...", "Ctrl+,") {
            self.settings_open = true;
        }
        ui.separator();
        if item(ui, "Exit", "Alt+F4") {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn capture_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut act: Option<Action> = None;
        let mut pick: Option<CaptureKind> = None;
        let hk = |s: &Settings, a: Action| {
            let c = s.binding(a);
            if c.is_set() { c.label() } else { String::new() }
        };
        for a in [Action::AllInOne, Action::Area, Action::Window, Action::Screen, Action::AllScreens, Action::ActiveWindow] {
            if item(ui, a.label(), &hk(&self.settings, a)) {
                act = Some(a);
            }
        }
        if item_on(ui, self.last_region.is_some(), Action::LastArea.label(), &hk(&self.settings, Action::LastArea)) {
            act = Some(Action::LastArea);
        }
        ui.menu_button("Fun shapes", |ui| {
            for s in CutShape::ALL {
                if item(ui, s.label(), "") {
                    pick = Some(CaptureKind::Pick(s, PickPurpose::Capture));
                }
            }
        });
        if item(ui, Action::Scrolling.label(), &hk(&self.settings, Action::Scrolling)) {
            act = Some(Action::Scrolling);
        }
        if item(ui, Action::Panorama.label(), &hk(&self.settings, Action::Panorama)) {
            act = Some(Action::Panorama);
        }
        for a in [Action::ManualScroll, Action::MultiArea, Action::MenuCapture] {
            if item(ui, a.label(), &hk(&self.settings, a)) {
                act = Some(a);
            }
        }
        if item(ui, "Capture from the clipboard", "") {
            self.paste();
        }
        if item(ui, "Capture from a file...", "") {
            self.open_dialog();
        }
        ui.menu_button("Fixed size", |ui| {
            for (w, h) in [(640, 480), (800, 600), (1024, 768), (1280, 720), (1920, 1080), (500, 500)] {
                if item(ui, &format!("{w} x {h}"), "") {
                    pick = Some(CaptureKind::Fixed(w, h));
                }
            }
        });
        ui.separator();
        ui.menu_button("Screens", |ui| {
            if self.monitors_at.is_none_or(|t| t.elapsed().as_secs_f32() > 2.0) {
                self.monitors = capture::monitors();
                self.monitors_at = Some(Instant::now());
            }
            for (n, m) in self.monitors.iter().enumerate() {
                let label = format!("{}: {}  {} x {}{}", n + 1, m.name, m.width, m.height, if m.primary { "  (main)" } else { "" });
                if ui.button(label).clicked() {
                    pick = Some(CaptureKind::Monitor(m.id));
                    ui.close();
                }
            }
        });
        ui.menu_button("Window list", |ui| {
            if self.windows_at.is_none_or(|t| t.elapsed().as_secs_f32() > 2.0) {
                self.windows = capture::list_windows();
                self.windows_at = Some(Instant::now());
            }
            if self.windows.is_empty() {
                ui.label(RichText::new("No windows found").color(TEXT_DIM));
            }
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for w in &self.windows {
                    if ui.button(&w.label).clicked() {
                        pick = Some(CaptureKind::Window(w.id, w.label.clone()));
                        ui.close();
                    }
                }
            });
        });
        ui.menu_button("Presets", |ui| {
            let mut run = None;
            for (i, p) in self.settings.presets.iter().enumerate() {
                let keys = if p.combo.is_set() { p.combo.label() } else { String::new() };
                if item(ui, &p.name, &keys) {
                    run = Some(i);
                }
            }
            ui.separator();
            if item(ui, "Edit presets...", "") {
                self.settings_open = true;
                self.settings_tab = SettingsTab::Presets;
            }
            if item(ui, "Import presets...", "") {
                self.import_presets();
            }
            if item(ui, "Export all presets...", "") {
                self.export_presets(None);
            }
            if let Some(i) = run {
                self.run_preset(ctx, i);
            }
        });
        ui.separator();
        ui.menu_button("Delay", |ui| {
            for d in [0u64, 1, 2, 3, 5, 10, 15, 30] {
                let label = if d == 0 { "No delay".to_string() } else { format!("{d} seconds") };
                if ui.radio(self.settings.delay_s == d, label).clicked() {
                    self.settings.delay_s = d;
                    ui.close();
                }
            }
        });
        if item(ui, "Timer: scheduled and repeated captures...", "") {
            self.timer_open = true;
        }
        ui.menu_button("After capture", |ui| {
            ui.checkbox(&mut self.settings.after_show_editor, "Show Snagster with the picture");
            ui.checkbox(&mut self.settings.after_library, "Add to the library");
            ui.checkbox(&mut self.settings.after_copy, "Copy to clipboard");
            ui.checkbox(&mut self.settings.after_save, "Save to the save folder");
            ui.checkbox(&mut self.settings.after_pin, "Pin to screen");
        });
        ui.checkbox(&mut self.settings.include_cursor, "Include the mouse pointer (movable)");
        ui.checkbox(&mut self.settings.hide_window, "Hide Snagster while capturing");
        if let Some(a) = act {
            ui.close();
            self.do_action(ctx, a);
        }
        if let Some(k) = pick {
            ui.close();
            self.start_capture(ctx, k);
        }
    }

    fn record_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let hk = |s: &Settings, a: Action| {
            let c = s.binding(a);
            if c.is_set() { c.label() } else { String::new() }
        };
        let gif_label = if self.mini == Some(Mini::Gif) { "Stop GIF recording" } else { "Record an area as GIF..." };
        if item(ui, gif_label, &hk(&self.settings, Action::RecordGif)) {
            self.do_action(ctx, Action::RecordGif);
        }
        let step_label = if self.mini == Some(Mini::Steps) { "Stop step recording" } else { "Record steps (a picture per click)" };
        if item(ui, step_label, &hk(&self.settings, Action::RecordSteps)) {
            self.do_action(ctx, Action::RecordSteps);
        }
        ui.separator();
        if item(ui, "GIF recordings: trim, frames, combine...", "") {
            self.gif_open = true;
        }
        if item_on(ui, self.entries.len() >= 2, "Make a GIF slideshow from Recent", "") {
            self.slideshow(ctx);
        }
    }

    pub(crate) fn slideshow(&mut self, ctx: &egui::Context) {
        let imgs: Vec<RgbaImage> = self.entries.iter().map(|e| render::compose(&e.doc)).collect();
        let frames = export::letterbox(&imgs, self.settings.gif_max_width);
        let ms = (self.settings.slideshow_s.clamp(0.2, 30.0) * 1000.0) as u32;
        let folder = PathBuf::from(&self.settings.library_folder).join("Recordings");
        let name = chrono::Local::now().format("Slideshow_%Y-%m-%d_%H-%M-%S").to_string();
        let path = crate::settings::unique_path(&folder, &name, "gif");
        let list: Vec<(RgbaImage, u32)> = frames.into_iter().map(|f| (f, ms)).collect();
        match export::write_gif(&path, &list) {
            Ok(()) => {
                self.recordings.insert(0, Recording { path: path.clone(), frames: list.len() as u32, seconds: list.len() as f32 * ms as f32 / 1000.0 });
                self.status = format!("Made a {}-picture slideshow GIF.", list.len());
                self.open_gif(ctx, path);
            }
            Err(e) => self.status = format!("Couldn't make the slideshow: {e}"),
        }
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        let (can_undo, can_redo, has_marks) = self
            .current
            .map(|i| {
                let e = &self.entries[i];
                (!e.undo.is_empty(), !e.redo.is_empty(), !e.doc.shapes.is_empty())
            })
            .unwrap_or((false, false, false));
        if item(ui, "Editor preferences...", "Ctrl+,") {
            self.settings_open = true;
        }
        ui.separator();
        if item_on(ui, can_undo, "Undo", "Ctrl+Z") {
            self.undo();
        }
        if item_on(ui, can_redo, "Redo", "Ctrl+Y") {
            self.redo();
        }
        ui.separator();
        let sel = self.selected.is_some();
        if item_on(ui, sel, "Delete mark", "Del") {
            self.delete_selected();
        }
        if item_on(ui, sel, "Make mark bigger", "]") {
            self.scale_selected(1.15);
        }
        if item_on(ui, sel, "Make mark smaller", "[") {
            self.scale_selected(1.0 / 1.15);
        }
        if item_on(ui, has_marks, "Clear all marks", "") {
            self.clear_marks();
        }
        ui.separator();
        let has = self.current.is_some();
        let sel = self.selected.is_some();
        if item_on(ui, sel, "Cut mark", "Ctrl+X") {
            self.cut_mark();
        }
        if item_on(ui, sel, "Copy mark", "Ctrl+C") {
            self.copy_mark();
        }
        if item_on(ui, self.mark_clip.is_some() && has, "Paste mark", "Ctrl+V") {
            self.paste_mark();
        }
        if item_on(ui, has, "Paste a picture onto this one", "") {
            self.paste_as_piece();
        }
        if item_on(ui, has, "Grab text...", "") {
            if let Some(i) = self.current {
                let img = render::compose(&self.entries[i].doc);
                self.run_ocr(OcrPurpose::GrabText, img);
            }
        }
        ui.separator();
        if item_on(ui, has, "Duplicate picture", "") {
            self.duplicate();
        }
        if item_on(ui, has, "Remove picture from Recent", "") {
            self.remove_current();
        }
        ui.separator();
        ui.menu_button("Tool", |ui| {
            for t in Tool::ALL {
                if ui
                    .add(egui::Button::new(t.label()).selected(self.tool == t).shortcut_text(t.key()))
                    .clicked()
                {
                    self.commit_text();
                    self.tool = t;
                    ui.close();
                }
            }
        });
    }

    fn image_menu(&mut self, ui: &mut egui::Ui) {
        let has = self.current.is_some();
        for e in Effect::ALL {
            if e.starts_group() {
                ui.separator();
            }
            let label = match e {
                Effect::Resize => format!("Resize to {}%", self.settings.resize_percent),
                Effect::Border => format!("Add {} px border", self.settings.border_width),
                _ => e.label().to_string(),
            };
            if item_on(ui, has, &label, "") {
                self.apply_effect(e);
            }
        }
        ui.separator();
        let busy = self.ocr_busy;
        if item_on(ui, has && !busy, "Smart redact (blur emails, phone numbers...)", "")
            && let Some(i) = self.current
        {
            let img = render::compose(&self.entries[i].doc);
            self.run_ocr(OcrPurpose::Redact(i), img);
        }
        if item_on(ui, has, "Smart redact settings...", "") {
            self.settings_open = true;
            self.settings_tab = SettingsTab::Redact;
        }
        if item_on(ui, has && !busy, "Simplify (text becomes bars)", "")
            && let Some(i) = self.current
        {
            let img = render::compose(&self.entries[i].doc);
            self.run_ocr(OcrPurpose::Simplify(i), img);
        }
        ui.separator();
        if item_on(ui, has, "Resize image...", "") {
            if let Some(i) = self.current {
                let (w, h) = self.entries[i].doc.image.dimensions();
                self.dialog = Some(crate::windows::Dialog::Resize { w, h, keep: true, ratio: w as f32 / h.max(1) as f32 });
            }
        }
        if item_on(ui, has, "Trim flat edges", "") {
            self.image_op("Trimmed", effects::trim);
        }
        if item_on(ui, has, "Canvas size and color...", "") {
            if let Some(i) = self.current {
                let (w, h) = self.entries[i].doc.image.dimensions();
                self.dialog = Some(crate::windows::Dialog::Canvas { w: w + 80, h: h + 80, color: [255, 255, 255, 255] });
            }
        }
        ui.separator();
        if item(ui, "Effect settings...", "") {
            self.settings_open = true;
            self.settings_tab = SettingsTab::Effects;
        }
    }

    fn tools_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let has = self.current.is_some();
        if item_on(ui, has && !self.ocr_busy, "Grab text from this picture", "")
            && let Some(i) = self.current
        {
            let img = render::compose(&self.entries[i].doc);
            self.run_ocr(OcrPurpose::GrabText, img);
        }
        if item(ui, "Grab text from the screen...", &self.settings.binding(Action::GrabText).label_if_set()) {
            ui.close();
            self.do_action(ctx, Action::GrabText);
        }
        if item(ui, "Screen color picker", "") {
            ui.close();
            self.do_action(ctx, Action::ColorPicker);
        }
        if item(ui, "Screen ruler", "") {
            ui.close();
            self.do_action(ctx, Action::Ruler);
        }
        ui.separator();
        if item_on(ui, has, "Pin picture to screen", "Ctrl+P") {
            self.pin_current();
        }
        let many = self.entries.len() >= 2;
        if item_on(ui, many, "Combine Recent side by side", "") {
            self.combine(false);
        }
        if item_on(ui, many, "Combine Recent top to bottom", "") {
            self.combine(true);
        }
        if item_on(ui, has, "One-page guide from Recent", "") {
            self.export_guide("page");
        }
        ui.separator();
        let c = self.settings.color;
        if item(ui, &format!("Copy color #{:02X}{:02X}{:02X}", c[0], c[1], c[2]), "") {
            let hex = format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
            let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(hex.clone()));
            self.status = format!("Copied {hex}.");
        }
        if !self.pins.is_empty() && item(ui, "Close all pins", "") {
            for p in &mut self.pins {
                p.close = true;
            }
        }
    }

    fn share_menu(&mut self, ui: &mut egui::Ui) {
        let has = self.current.is_some();
        if item_on(ui, has, "Copy picture", "Ctrl+C") {
            self.copy();
        }
        if item_on(ui, has, "Email...", "") {
            self.email();
        }
        if item_on(ui, !self.entries.is_empty(), "Microsoft Word document...", "") {
            self.export_word();
        }
        if item_on(ui, has, "Print...", "") {
            self.print();
        }
        if item_on(ui, has, "Open in the default picture program", "") {
            self.open_externally();
        }
        if item_on(ui, has, "Save to the save folder", "Ctrl+Shift+S") {
            self.quick_save();
        }
        let path = self.current.and_then(|i| self.entries[i].path.clone());
        if item_on(ui, path.is_some(), "Show the file in its folder", "")
            && let Some(p) = path
        {
            self.open_folder(&p);
        }
        ui.separator();
        ui.label(RichText::new("Cloud folders (point the library or save folder here to sync):").color(TEXT_DIM));
        let clouds = crate::settings::cloud_folders();
        if clouds.is_empty() {
            ui.label(RichText::new("No OneDrive, Dropbox, Google Drive or iCloud folder found.").color(TEXT_DIM));
        }
        for (name, p) in clouds {
            if item_on(ui, has, &format!("Save to {name}"), "")
                && let Some(i) = self.current
            {
                let title = self.entries[i].name.clone();
                let base = self.settings.file_name(&title);
                let path = crate::settings::unique_path(&p.join("Snagster"), &base, self.settings.format.ext());
                self.save_to(i, path);
            }
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if item(ui, "Fit to window", "0") {
            self.set_zoom(ctx, None);
        }
        if item(ui, "Actual size", "1") {
            self.set_zoom(ctx, Some(0.0));
        }
        if item(ui, "Zoom in", "+") {
            self.set_zoom(ctx, Some(1.25));
        }
        if item(ui, "Zoom out", "-") {
            self.set_zoom(ctx, Some(0.8));
        }
        ui.separator();
        ui.menu_button("Customize toolbar", |ui| {
            for t in Tool::ALL {
                let mut on = !self.settings.hidden_tools.iter().any(|h| h == t.label());
                if ui.checkbox(&mut on, t.label()).changed() {
                    if on {
                        self.settings.hidden_tools.retain(|h| h != t.label());
                    } else {
                        self.settings.hidden_tools.push(t.label().to_string());
                    }
                }
            }
            if ui.button("Show all").clicked() {
                self.settings.hidden_tools.clear();
            }
        });
        ui.checkbox(&mut self.settings.show_side_panel, "Show options panel");
        ui.checkbox(&mut self.settings.show_tray, "Show Recent pictures");
        if item(ui, "Capture panel", "") {
            self.enter_panel(ctx);
        }
    }

    // ---------- panels ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.add_space(6.0);
            logo(ui, 26.0);
            ui.label(RichText::new("Snagster").size(20.0).strong().color(Color32::WHITE));
            ui.add_space(12.0);
            let busy = self.capturing.is_some();
            let ctx = ui.ctx().clone();
            let primary = egui::Button::new(RichText::new("Capture").color(Color32::WHITE).strong())
                .fill(ACCENT)
                .min_size(vec2(0.0, 30.0));
            if ui
                .add_enabled(!busy, primary)
                .on_hover_text("All-in-one: drag an area, click a window, button or menu, then capture, scroll, record or grab text (Print Screen)")
                .clicked()
            {
                self.do_action(&ctx, Action::AllInOne);
            }
            if ui.add_enabled(!busy, egui::Button::new("Window").min_size(vec2(0.0, 30.0))).clicked() {
                self.do_action(&ctx, Action::Window);
            }
            if ui.add_enabled(!busy, egui::Button::new("Screen").min_size(vec2(0.0, 30.0))).clicked() {
                self.do_action(&ctx, Action::Screen);
            }
            if ui.add_enabled(!busy, egui::Button::new("Scrolling").min_size(vec2(0.0, 30.0))).clicked() {
                self.do_action(&ctx, Action::Scrolling);
            }
            let rec = if self.mini == Some(Mini::Gif) { "Stop GIF" } else { "Record GIF" };
            if ui.add(egui::Button::new(rec).min_size(vec2(0.0, 30.0))).clicked() {
                self.do_action(&ctx, Action::RecordGif);
            }
            ui.label(RichText::new("Delay").color(TEXT_DIM));
            egui::ComboBox::from_id_salt("delay")
                .width(56.0)
                .selected_text(if self.settings.delay_s == 0 { "None".to_string() } else { format!("{} s", self.settings.delay_s) })
                .show_ui(ui, |ui| {
                    for d in [0u64, 1, 2, 3, 5, 10, 15, 30] {
                        let label = if d == 0 { "None".to_string() } else { format!("{d} s") };
                        ui.selectable_value(&mut self.settings.delay_s, d, label);
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let has = self.current.is_some();
                let save = egui::Button::new(RichText::new("Save").color(Color32::WHITE).strong())
                    .fill(Color32::from_rgb(48, 164, 108))
                    .min_size(vec2(60.0, 30.0));
                if ui.add_enabled(has, save).on_hover_text("Save as a picture or project (Ctrl+S)").clicked() {
                    self.save_as();
                }
                if ui.add_enabled(has, egui::Button::new("Copy").min_size(vec2(52.0, 30.0))).clicked() {
                    self.copy();
                }
                if ui.add_enabled(has, egui::Button::new("Pin").min_size(vec2(40.0, 30.0))).clicked() {
                    self.pin_current();
                }
                ui.separator();
                let (can_redo, can_undo) = self
                    .current
                    .map(|i| (!self.entries[i].redo.is_empty(), !self.entries[i].undo.is_empty()))
                    .unwrap_or((false, false));
                if ui.add_enabled(can_redo, egui::Button::new("Redo")).on_hover_text("Ctrl+Y").clicked() {
                    self.redo();
                }
                if ui.add_enabled(can_undo, egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                    self.undo();
                }
                ui.separator();
                if ui.button("Panel").on_hover_text("The compact capture panel (also from the icon in the taskbar corner)").clicked() {
                    let c = ui.ctx().clone();
                    self.enter_panel(&c);
                }
                if ui.button("Library").on_hover_text("Every capture, searchable (Ctrl+L)").clicked() {
                    self.library_open = true;
                }
                if ui.button("Settings").on_hover_text("Ctrl+,").clicked() {
                    self.settings_open = true;
                }
            });
        });
    }

    fn tool_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        egui::Grid::new("tools").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            let shown: Vec<Tool> = Tool::ALL.into_iter().filter(|t| !self.settings.hidden_tools.iter().any(|h| h == t.label())).collect();
            for (n, t) in shown.into_iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(54.0, 44.0), Sense::click());
                let selected = self.tool == t;
                let bg = if selected {
                    ACCENT
                } else if resp.hovered() {
                    PANEL_HI
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect.shrink(1.0), CornerRadius::same(8), bg);
                let fg = if selected { Color32::WHITE } else { Color32::from_rgb(210, 214, 225) };
                let icon = Rect::from_center_size(rect.center() - vec2(0.0, 7.0), vec2(20.0, 20.0));
                tool_icon(ui.painter(), icon, t, fg, self.color());
                ui.painter().text(
                    pos2(rect.center().x, rect.bottom() - 8.0),
                    Align2::CENTER_CENTER,
                    t.label(),
                    FontId::proportional(10.0),
                    fg,
                );
                let resp = resp.on_hover_text(format!("{} ({})\n{}", t.label(), t.key(), t.hint()));
                if resp.clicked() {
                    self.commit_text();
                    self.tool = t;
                    if t != Tool::Select {
                        self.selected = None;
                    }
                }
                if n % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(10.0);
            section(ui, "Color");
            let mut chosen = None;
            let cur = self.color();
            let palette = self.settings.palette;
            let mut edit_slot: Option<(usize, Color32)> = None;
            egui::Grid::new("palette").spacing(vec2(6.0, 6.0)).show(ui, |ui| {
                for (i, c) in palette.iter().enumerate() {
                    let c = rgb(*c);
                    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 26.0), Sense::click());
                    ui.painter().rect_filled(rect, CornerRadius::same(6), c);
                    if c == cur {
                        ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(8), Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
                    } else if resp.hovered() {
                        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, TEXT_DIM), StrokeKind::Outside);
                    }
                    if resp.clicked() {
                        chosen = Some(c);
                    }
                    resp.on_hover_text("Click to use. Right-click puts the current color here.").context_menu(|ui| {
                        if ui.button("Put the current color here").clicked() {
                            edit_slot = Some((i, cur));
                            ui.close();
                        }
                    });
                    if i % 4 == 3 {
                        ui.end_row();
                    }
                }
            });
            if let Some((i, c)) = edit_slot {
                self.settings.palette[i] = [c.r(), c.g(), c.b()];
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut c = cur;
                if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
                    chosen = Some(c);
                }
                ui.label(RichText::new(format!("#{:02X}{:02X}{:02X}", cur.r(), cur.g(), cur.b())).color(TEXT_DIM));
            });
            if let Some(c) = chosen {
                self.set_color(c);
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Theme").color(TEXT_DIM));
                let mut apply = None;
                egui::ComboBox::from_id_salt("theme").width(120.0).selected_text("Choose...").show_ui(ui, |ui| {
                    for (i, t) in self.settings.themes.iter().enumerate() {
                        if ui.button(&t.name).clicked() {
                            apply = Some(i);
                        }
                    }
                });
                if let Some(i) = apply {
                    let t = self.settings.themes[i].clone();
                    self.settings.palette = t.palette;
                    self.settings.color = t.palette[0];
                    self.settings.font = t.font;
                    self.settings.text_background = t.text_background;
                    self.settings.shadow = t.shadow;
                    self.status = format!("Theme \"{}\" applied.", t.name);
                }
            });
            ui.add_space(10.0);
            section(ui, "Size");
            ui.add(egui::Slider::new(&mut self.settings.width, 1.0..=24.0).step_by(1.0).suffix(" px"));

            let t = self.tool;
            ui.add_space(6.0);
            if matches!(t, Tool::Arrow | Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Pen | Tool::Text | Tool::Callout | Tool::Step) {
                ui.checkbox(&mut self.settings.shadow, "Drop shadow");
            }
            if matches!(t, Tool::Rect | Tool::Ellipse) {
                ui.checkbox(&mut self.settings.fill, "Fill boxes and ovals");
            }
            if t == Tool::Rect {
                ui.label(RichText::new("Rounded corners").color(TEXT_DIM));
                ui.add(egui::Slider::new(&mut self.settings.corner_radius, 0.0..=60.0).step_by(1.0).suffix(" px"));
            }
            if matches!(t, Tool::Text | Tool::Callout | Tool::Select) {
                self.font_picker(ui);
                if t == Tool::Text {
                    ui.checkbox(&mut self.settings.text_background, "Text on a colored box");
                }
                ui.label(RichText::new(format!("Text size {:.0} px", doc::text_size(self.settings.width))).color(TEXT_DIM));
            }
            if t == Tool::Blur {
                ui.label(RichText::new("Pixel block size").color(TEXT_DIM));
                ui.add(egui::Slider::new(&mut self.settings.blur_block, 4..=48).suffix(" px"));
            }
            if t == Tool::Magnify {
                ui.label(RichText::new("Magnification").color(TEXT_DIM));
                ui.add(egui::Slider::new(&mut self.settings.magnify_zoom, 1.5..=6.0).suffix(" x"));
            }
            if t == Tool::Step {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Style").color(TEXT_DIM));
                    egui::ComboBox::from_id_salt("stepstyle").selected_text(self.settings.step_style.label()).show_ui(ui, |ui| {
                        for s in crate::settings::StepStyle::ALL {
                            ui.selectable_value(&mut self.settings.step_style, s, s.label());
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("First").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut self.settings.step_start).range(1..=999));
                });
                if let Some(i) = self.current {
                    let n = self.entries[i].doc.next_step(self.settings.step_start);
                    ui.label(RichText::new(format!("Next: {}", self.settings.step_style.format(n))).color(TEXT_DIM));
                }
            }
            if t == Tool::Stamp {
                self.stamp_picker(ui);
            }
            self.extra_tool_options(ui);

            if self.selected.is_some() {
                ui.add_space(10.0);
                section(ui, "Selected mark");
                ui.label(RichText::new("Pick a color to recolor it. [ and ] change its size.").color(TEXT_DIM));
                ui.horizontal(|ui| {
                    if ui.button("Smaller").clicked() {
                        self.scale_selected(1.0 / 1.15);
                    }
                    if ui.button("Bigger").clicked() {
                        self.scale_selected(1.15);
                    }
                    if ui.button("Delete").clicked() {
                        self.delete_selected();
                    }
                });
            }

            ui.add_space(10.0);
            section(ui, "View");
            let ctx = ui.ctx().clone();
            ui.horizontal(|ui| {
                if ui.selectable_label(self.zoom.is_none(), "Fit").clicked() {
                    self.set_zoom(&ctx, None);
                }
                if ui.button("100%").clicked() {
                    self.set_zoom(&ctx, Some(0.0));
                }
                if ui.button(" - ").clicked() {
                    self.set_zoom(&ctx, Some(0.8));
                }
                if ui.button(" + ").clicked() {
                    self.set_zoom(&ctx, Some(1.25));
                }
            });
            let ppp = ui.ctx().pixels_per_point();
            ui.label(RichText::new(format!("Zoom {:.0}%", self.last_scale * ppp * 100.0)).color(TEXT_DIM));
            if let Some(i) = self.current {
                let d = &self.entries[i].doc;
                ui.label(RichText::new(format!("{} x {} pixels, {} marks", d.image.width(), d.image.height(), d.shapes.len())).color(TEXT_DIM));
            }
            ui.add_space(10.0);
            ui.label(RichText::new(self.tool.label()).strong().color(Color32::WHITE));
            ui.add(egui::Label::new(RichText::new(self.tool.hint()).color(TEXT_DIM).size(12.5)).wrap());
        });
    }

    fn font_picker(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Font").color(TEXT_DIM));
        let shown = if self.settings.font.is_empty() { "Default".to_string() } else { self.settings.font.clone() };
        let families = fonts::families();
        let mut chosen: Option<String> = None;
        egui::ComboBox::from_id_salt("font").width(180.0).selected_text(shown).show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.font_query).hint_text("Search fonts"));
            if ui.selectable_label(self.settings.font.is_empty(), "Default").clicked() {
                chosen = Some(String::new());
            }
            match &families {
                None => {
                    ui.label(RichText::new("Reading the installed fonts...").color(TEXT_DIM));
                }
                Some(list) => {
                    let q = self.font_query.to_lowercase();
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for f in list.iter().filter(|f| q.is_empty() || f.to_lowercase().contains(&q)) {
                            if ui.selectable_label(self.settings.font == *f, f).clicked() {
                                chosen = Some(f.clone());
                            }
                        }
                    });
                }
            }
        });
        if let Some(n) = &families {
            ui.label(RichText::new(format!("{} fonts installed", n.len())).color(TEXT_DIM).size(11.5));
        }
        ui.horizontal(|ui| {
            ui.toggle_value(&mut self.bold, RichText::new("B").strong());
            ui.toggle_value(&mut self.italic, RichText::new("I").italics());
        });
        if let Some(f) = chosen {
            self.settings.font = f.clone();
            if let Some(t) = &mut self.text_edit {
                t.font = f.clone();
            }
            // Re-font the selected text mark too.
            if let Some(s) = self.selected
                && let Some(e) = self.entry()
                && let Some(Shape::Text { font, .. } | Shape::Callout { font, .. }) = e.doc.shapes.get_mut(s)
            {
                *font = f;
            }
        }
        if let Some(t) = &mut self.text_edit {
            t.bold = self.bold;
            t.italic = self.italic;
        }
    }

    fn stamp_picker(&mut self, ui: &mut egui::Ui) {
        let ef = crate::emoji::font();
        ui.label(
            RichText::new(if ef.color {
                format!("{} emojis in color", ef.count())
            } else {
                format!("{} emojis (black and white)", ef.count())
            })
            .color(TEXT_DIM),
        );
        let groups: Vec<&str> = ef.groups.iter().map(|g| g.0).collect();
        let gi = self.stamp_group.min(groups.len().saturating_sub(1));
        egui::ComboBox::from_id_salt("stampgroup").width(180.0).selected_text(groups.get(gi).copied().unwrap_or("")).show_ui(ui, |ui| {
            for (i, g) in groups.iter().enumerate() {
                ui.selectable_value(&mut self.stamp_group, i, *g);
            }
        });
        let ctx = ui.ctx().clone();
        if ui.selectable_label(self.stamp == POINTER_KEY.to_string(), "Mouse pointer").clicked() {
            self.stamp = POINTER_KEY.to_string();
        }
        let list = ef.groups.get(gi).map(|g| g.1.clone()).unwrap_or_default();
        egui::ScrollArea::vertical().max_height(300.0).id_salt("stamps").show(ui, |ui| {
            egui::Grid::new("stampgrid").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
                for (n, ch) in list.iter().enumerate() {
                    let tex = self.stamp_texture(&ctx, *ch, 32, [40, 40, 40, 255]);
                    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                    if self.stamp.starts_with(*ch) {
                        ui.painter().rect_filled(rect, CornerRadius::same(4), ACCENT);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(4), PANEL_HI);
                    }
                    if let Some(t) = tex {
                        ui.painter().image(t, rect.shrink(3.0), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    if resp.on_hover_text(format!("U+{:04X}", *ch as u32)).clicked() {
                        self.stamp = ch.to_string();
                    }
                    if n % 5 == 4 {
                        ui.end_row();
                    }
                }
            });
        });
    }

    fn stamp_texture(&mut self, ctx: &egui::Context, ch: char, px: u32, tint: [u8; 4]) -> Option<TextureId> {
        let px = px.next_power_of_two().clamp(16, 512);
        if let Some(t) = self.stamp_tex.get(&(ch, px)) {
            return Some(t.id());
        }
        let img = if ch == POINTER_KEY { Arc::new(pointer_image(px)) } else { crate::emoji::render(ch, px, tint)? };
        let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
        let h = ctx.load_texture(format!("stamp-{}-{px}", ch as u32), ci, TextureOptions::LINEAR);
        let id = h.id();
        self.stamp_tex.insert((ch, px), h);
        Some(id)
    }

    fn tray(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width(250.0);
                ui.add_space(8.0);
                ui.label(RichText::new("Recent").strong().color(Color32::WHITE));
                ui.add(egui::Label::new(RichText::new(&self.status).color(TEXT_DIM).size(12.5)).wrap());
            });
            ui.separator();
            let mut switch = None;
            let mut remove = None;
            let mut pin = None;
            let ctx = ui.ctx().clone();
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.entries.is_empty() {
                        ui.label(RichText::new("Your captures show up here.").color(TEXT_DIM));
                    }
                    for i in (0..self.entries.len()).rev() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(124.0, 84.0), Sense::click());
                        let tex = self.entries[i].texture(&ctx);
                        let is_cur = self.current == Some(i);
                        let thumb_area = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(116.0, 58.0));
                        let size = self.entries[i].doc.size();
                        let s = (thumb_area.width() / size.x).min(thumb_area.height() / size.y);
                        let thumb = Rect::from_center_size(thumb_area.center(), size * s);
                        let shapes = std::mem::take(&mut self.entries[i].doc.shapes);
                        let pc = self.prepare_paint(&ctx, &shapes, s, Some((tex, size)));
                        let p = ui.painter();
                        p.rect_filled(rect, CornerRadius::same(8), if is_cur { PANEL_HI } else { BG_DARK });
                        p.image(tex, thumb, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                        let to = |q: Pos2| pos2(thumb.min.x + q.x * s, thumb.min.y + q.y * s);
                        for sh in &shapes {
                            paint_shape(p, sh, &to, s, thumb, &pc);
                        }
                        self.entries[i].doc.shapes = shapes;
                        if is_cur {
                            p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(2.0, ACCENT), StrokeKind::Inside);
                        } else if resp.hovered() {
                            p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, TEXT_DIM), StrokeKind::Inside);
                        }
                        let name: String = self.entries[i].name.chars().take(18).collect();
                        p.text(pos2(rect.center().x, rect.bottom() - 10.0), Align2::CENTER_CENTER, name, FontId::proportional(11.5), Color32::from_rgb(210, 214, 225));
                        if resp.clicked() {
                            switch = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Pin to screen").clicked() {
                                pin = Some(i);
                                ui.close();
                            }
                            if ui.button("Remove from Recent").clicked() {
                                remove = Some(i);
                                ui.close();
                            }
                        });
                    }
                });
            });
            if let Some(i) = switch.or(pin) {
                self.commit_text();
                self.current = Some(i);
                self.zoom = None;
                self.pan = Vec2::ZERO;
                self.selected = None;
                self.drag = Drag::None;
            }
            if pin.is_some() {
                self.pin_current();
            }
            if let Some(i) = remove {
                self.current = Some(i);
                self.remove_current();
            }
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let area = ui.max_rect();
        let card_w = 180.0;
        let total = card_w * 4.0 + 60.0;
        let top = area.center().y - 180.0;
        let p = ui.painter();
        logo_at(p, pos2(area.center().x, top), 64.0);
        p.text(pos2(area.center().x, top + 62.0), Align2::CENTER_CENTER, "Snagster", FontId::proportional(40.0), Color32::WHITE);
        p.text(pos2(area.center().x, top + 100.0), Align2::CENTER_CENTER, "Capture your screen, mark it up, share it.", FontId::proportional(17.0), TEXT_DIM);
        let ctx = ui.ctx().clone();
        for k in 0..4usize {
            let (title, sub) = [
                ("Capture", "Print Screen"),
                ("Scrolling page", "Long pages and documents"),
                ("Record a GIF", "Show what happens"),
                ("Open a picture", "Or a .snag project"),
            ][k];
            let x = area.center().x - total / 2.0 + k as f32 * (card_w + 20.0);
            let rect = Rect::from_min_size(pos2(x, top + 140.0), vec2(card_w, 120.0));
            let resp = ui.interact(rect, ui.scope_id().with(("card", k)), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(14), if resp.hovered() { PANEL_HI } else { PANEL });
            if k == 0 {
                p.rect_stroke(rect, CornerRadius::same(14), Stroke::new(2.0, ACCENT), StrokeKind::Inside);
            }
            let icon = Rect::from_center_size(rect.center() - vec2(0.0, 22.0), vec2(30.0, 30.0));
            match k {
                0 => frame_icon(p, icon, ACCENT, true),
                1 => {
                    frame_icon(p, icon, ACCENT, false);
                    p.arrow(icon.center() - vec2(0.0, 6.0), vec2(0.0, 12.0), Stroke::new(2.0, ACCENT));
                }
                2 => {
                    p.circle_filled(icon.center(), 9.0, Color32::from_rgb(229, 72, 77));
                    p.circle_stroke(icon.center(), 14.0, Stroke::new(2.0, ACCENT));
                }
                _ => folder_icon(p, icon, ACCENT),
            }
            p.text(rect.center() + vec2(0.0, 16.0), Align2::CENTER_CENTER, title, FontId::proportional(16.0), Color32::WHITE);
            p.text(rect.center() + vec2(0.0, 38.0), Align2::CENTER_CENTER, sub, FontId::proportional(12.5), TEXT_DIM);
            if resp.clicked() {
                match k {
                    0 => self.do_action(&ctx, Action::AllInOne),
                    1 => self.do_action(&ctx, Action::Scrolling),
                    2 => self.do_action(&ctx, Action::RecordGif),
                    _ => self.open_dialog(),
                }
            }
        }
        let link = Rect::from_center_size(pos2(area.center().x, top + 300.0), vec2(260.0, 26.0));
        let resp = ui.interact(link, ui.scope_id().with("sample"), Sense::click());
        let col = if resp.hovered() { Color32::WHITE } else { ACCENT };
        ui.painter().text(link.center(), Align2::CENTER_CENTER, "Try the tools on a sample picture", FontId::proportional(14.0), col);
        if resp.clicked() {
            let name = self.next_name("Sample");
            self.add_entry(name, sample::sample_doc());
        }
        ui.painter().text(pos2(area.center().x, top + 340.0), Align2::CENTER_CENTER, "Or drop a picture file here, or paste one with Ctrl+V.", FontId::proportional(12.5), TEXT_DIM);
    }

    /// Load the fonts and stamp pictures the marks need before they're drawn.
    fn prepare_paint(&mut self, ctx: &egui::Context, shapes: &[Shape], scale: f32, pic: Option<(TextureId, Vec2)>) -> PaintCtx {
        let mut pc = PaintCtx { families: HashMap::new(), stamps: HashMap::new(), pic, images: HashMap::new() };
        let ppp = ctx.pixels_per_point();
        for s in shapes {
            match s {
                Shape::Text { font, bold, italic, .. } | Shape::Callout { font, bold, italic, .. } => {
                    let k = fonts::key(font, *bold, *italic);
                    if !pc.families.contains_key(&k) {
                        let fam = self.ui_fonts.family(ctx, font, *bold, *italic);
                        pc.families.insert(k, fam);
                    }
                }
                Shape::Stamp { emoji, size, color, .. } => {
                    if let Some(ch) = emoji.chars().next() {
                        let px = ((size * scale * ppp).max(16.0) as u32).next_power_of_two().clamp(16, 512);
                        let [r, g, b, _] = color.to_srgba_unmultiplied();
                        if let Some(t) = self.stamp_texture(ctx, ch, px, [r, g, b, 255]) {
                            pc.stamps.insert(ch, t);
                        }
                    }
                }
                Shape::Pointer { size, .. } => {
                    let px = ((size * scale * ppp).max(16.0) as u32).next_power_of_two().clamp(16, 512);
                    if let Some(t) = self.stamp_texture(ctx, POINTER_KEY, px, [0, 0, 0, 255]) {
                        pc.stamps.insert(POINTER_KEY, t);
                    }
                }
                Shape::Image { image, .. } => {
                    let key = Arc::as_ptr(&image.0) as usize;
                    if !self.image_tex.contains_key(&key) {
                        let img = &image.0;
                        let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                        let h = ctx.load_texture("piece", ci, TextureOptions::LINEAR);
                        self.image_tex.insert(key, (image.0.clone(), h));
                    }
                    if let Some((_, h)) = self.image_tex.get(&key) {
                        pc.images.insert(key, h.id());
                    }
                }
                _ => {}
            }
        }
        pc
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let Some(ci) = self.current else {
            self.welcome(ui);
            return;
        };
        let area = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        let size = self.entries[ci].doc.size();
        let fit = ((area.width() - 48.0) / size.x).min((area.height() - 48.0) / size.y).min(1.0 / ppp).max(0.02);
        let mut scale = self.zoom.unwrap_or(fit);
        let mut origin = area.center() - size * scale / 2.0 + self.pan;

        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        let shift = ui.input(|i| i.modifiers.shift);

        if resp.hovered() {
            let zd = ui.input(|i| i.zoom_delta());
            if zd != 1.0 {
                let new_scale = (scale * zd).clamp(0.02, 16.0);
                let cursor = ui.input(|i| i.pointer.hover_pos()).unwrap_or(area.center());
                let new_origin = cursor - (cursor - origin) * (new_scale / scale);
                self.pan = new_origin - (area.center() - size * new_scale / 2.0);
                self.zoom = Some(new_scale);
                scale = new_scale;
                origin = new_origin;
            } else {
                let sd = ui.input(|i| i.smooth_scroll_delta);
                if sd != Vec2::ZERO {
                    self.pan += sd;
                    origin += sd;
                }
            }
        }
        if resp.dragged_by(PointerButton::Middle) {
            self.pan += resp.drag_delta();
            origin += resp.drag_delta();
        }
        self.last_scale = scale;
        let to_img = |p: Pos2| pos2((p.x - origin.x) / scale, (p.y - origin.y) / scale);
        let to_scr = move |p: Pos2| pos2(origin.x + p.x * scale, origin.y + p.y * scale);
        let img_rect = Rect::from_min_size(origin, size * scale);

        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(pp) = pointer
        {
            self.commit_text();
            let p = to_img(pp);
            match self.tool {
                Tool::Select => {
                    self.selected = self.pick(p);
                    self.drag = match self.selected {
                        Some(idx) => Drag::Move { idx, last: p, pushed: false },
                        None => Drag::None,
                    };
                }
                Tool::Text | Tool::Step | Tool::Stamp | Tool::Fill => {}
                _ => self.drag = Drag::Draw { start: p, cur: p, points: vec![p] },
            }
        }
        if resp.dragged_by(PointerButton::Primary)
            && let Some(pp) = pointer
        {
            let p = to_img(pp);
            match &mut self.drag {
                Drag::Draw { cur, points, .. } => {
                    *cur = p;
                    if points.last().is_none_or(|l| l.distance(p) * scale >= 2.0) {
                        points.push(p);
                    }
                }
                Drag::Move { idx, last, pushed } => {
                    let (idx, delta) = (*idx, p - *last);
                    *last = p;
                    let first = !*pushed;
                    *pushed = true;
                    let e = &mut self.entries[ci];
                    if first {
                        e.push_undo();
                    }
                    if let Some(s) = e.doc.shapes.get_mut(idx) {
                        s.translate(delta);
                    }
                }
                Drag::None => {}
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            if let Drag::Draw { start, cur, points } = std::mem::replace(&mut self.drag, Drag::None) {
                self.finish_draw(start, cur, points, shift);
            }
            self.drag = Drag::None;
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some(pp) = pointer
        {
            let p = to_img(pp);
            let had_text = self.text_edit.is_some();
            self.commit_text();
            match self.tool {
                Tool::Select => self.selected = self.pick(p),
                Tool::Text if !had_text => {
                    let hit = self.pick(p).filter(|&i| matches!(self.entries[ci].doc.shapes[i], Shape::Text { .. }));
                    self.start_text(ci, p, hit);
                }
                Tool::Callout if !had_text => {
                    let hit = self.pick(p).filter(|&i| matches!(self.entries[ci].doc.shapes[i], Shape::Callout { .. }));
                    if hit.is_some() {
                        self.start_text(ci, p, hit);
                    } else {
                        let sz = doc::text_size(self.settings.width);
                        self.start_text_kind(ci, p + vec2(sz * 2.0, -sz * 3.0), None, TextKind::Callout { tail: p });
                    }
                }
                Tool::Step => {
                    let (color, radius, shadow) = (self.color(), doc::step_radius(self.settings.width), self.settings.shadow);
                    let (start, style) = (self.settings.step_start, self.settings.step_style);
                    let e = &mut self.entries[ci];
                    let n = e.doc.next_step(start);
                    e.push_undo();
                    e.doc.shapes.push(Shape::Step { center: p, n, color, radius, shadow, style });
                }
                Tool::Fill => {
                    let color = self.color();
                    let e = &mut self.entries[ci];
                    if let Some(d) = e.doc.flood_fill(p, color, 32) {
                        e.push_undo();
                        e.doc = d;
                    }
                }
                Tool::Stamp => {
                    let color = self.color();
                    let w = self.settings.width;
                    let shape = if self.stamp.starts_with(POINTER_KEY) {
                        Shape::Pointer { pos: p, size: doc::pointer_size(w) }
                    } else {
                        Shape::Stamp { center: p, emoji: self.stamp.clone(), size: doc::text_size(w) * doc::STAMP_FACTOR, color }
                    };
                    let e = &mut self.entries[ci];
                    e.push_undo();
                    e.doc.shapes.push(shape);
                }
                _ => {}
            }
        }
        if resp.double_clicked()
            && self.tool == Tool::Select
            && let Some(pp) = pointer
        {
            let p = to_img(pp);
            if let Some(i) = self
                .pick(p)
                .filter(|&i| matches!(self.entries[ci].doc.shapes[i], Shape::Text { .. } | Shape::Callout { .. }))
            {
                self.start_text(ci, p, Some(i));
            }
        }
        resp.context_menu(|ui| {
            if ui.button("Copy picture").clicked() {
                self.copy();
                ui.close();
            }
            if ui.button("Pin to screen").clicked() {
                self.pin_current();
                ui.close();
            }
            if self.selected.is_some() && ui.button("Delete mark").clicked() {
                self.delete_selected();
                ui.close();
            }
            if ui.button("Clear all marks").clicked() {
                self.clear_marks();
                ui.close();
            }
        });

        if resp.hovered() {
            let icon = match self.tool {
                Tool::Select => egui::CursorIcon::Default,
                Tool::Text => egui::CursorIcon::Text,
                _ => egui::CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        // Paint.
        let ctx = ui.ctx().clone();
        let tex = self.entries[ci].texture(&ctx);
        // Borrow the marks for drawing without copying them (put back below).
        let shapes = std::mem::take(&mut self.entries[ci].doc.shapes);
        let pc = self.prepare_paint(&ctx, &shapes, scale, Some((tex, size)));
        let painter = ui.painter_at(area);
        painter.rect_filled(img_rect.translate(vec2(0.0, 6.0)).expand(4.0), CornerRadius::same(6), Color32::from_black_alpha(90));
        painter.rect_filled(img_rect, CornerRadius::ZERO, Color32::from_gray(200));
        painter.image(tex, img_rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        let editing_index = self.text_edit.as_ref().and_then(|t| t.index);
        for (i, s) in shapes.iter().enumerate() {
            if Some(i) == editing_index {
                continue;
            }
            paint_shape(&painter, s, &to_scr, scale, img_rect, &pc);
        }
        if let Some(sel) = self.selected
            && let Some(s) = shapes.get(sel)
        {
            let b = s.bounds();
            let r = Rect::from_two_pos(to_scr(b.min), to_scr(b.max)).expand(4.0);
            dashed_rect(&painter, r, ACCENT);
        }
        self.entries[ci].doc.shapes = shapes;
        if let Drag::Draw { start, cur, points } = &self.drag {
            let (start, mut cur) = (*start, *cur);
            let (color, width, fill) = (self.color(), self.settings.width, self.settings.fill);
            let (radius, shadow) = (self.settings.corner_radius, self.settings.shadow);
            match self.tool {
                Tool::Arrow | Tool::Line if shift => cur = doc::snap45(start, cur),
                Tool::Rect | Tool::Ellipse if shift => cur = doc::square(start, cur),
                _ => {}
            }
            let preview = match self.tool {
                Tool::Arrow => Some(Shape::Arrow { a: start, b: cur, color, width, shadow, heads: self.settings.arrow_heads }),
                Tool::Line => Some(Shape::Line { a: start, b: cur, color, width, shadow }),
                Tool::Rect => Some(Shape::Rect { rect: Rect::from_two_pos(start, cur), color, width, fill, radius, shadow }),
                Tool::Ellipse => Some(Shape::Ellipse { rect: Rect::from_two_pos(start, cur), color, width, fill, shadow }),
                Tool::Pen => Some(Shape::Pen { points: points.clone(), color, width, shadow }),
                Tool::Spotlight => Some(Shape::Spotlight { rect: Rect::from_two_pos(start, cur) }),
                Tool::Magnify => Some(Shape::Magnify { center: start, radius: start.distance(cur), zoom: self.settings.magnify_zoom }),
                Tool::Callout => Some(Shape::Arrow { a: cur, b: start, color, width: 2.0, shadow: false, heads: 1 }),
                Tool::Highlight => Some(Shape::Highlight {
                    points: if shift { vec![start, pos2(cur.x, start.y)] } else { points.clone() },
                    color,
                    width,
                }),
                _ => None,
            };
            if let Some(s) = preview {
                paint_shape(&painter, &s, &to_scr, scale, img_rect, &pc);
            }
            let r = Rect::from_two_pos(to_scr(start), to_scr(cur));
            match self.tool {
                Tool::Blur => {
                    painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(120, 120, 140, 90));
                    dashed_rect(&painter, r, Color32::WHITE);
                }
                Tool::Eraser => {
                    let pts: Vec<Pos2> = points.iter().map(|q| to_scr(*q)).collect();
                    painter.add(egui::Shape::line(pts, Stroke::new((self.settings.width * 3.0 * scale).max(1.0), Color32::from_white_alpha(120))));
                }
                Tool::Selection => {
                    dashed_rect(&painter, r, Color32::WHITE);
                    painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(79, 124, 255, 40));
                }
                Tool::CutOut => {
                    let band = if r.width() >= r.height() {
                        Rect::from_min_max(pos2(img_rect.min.x, r.min.y), pos2(img_rect.max.x, r.max.y))
                    } else {
                        Rect::from_min_max(pos2(r.min.x, img_rect.min.y), pos2(r.max.x, img_rect.max.y))
                    };
                    painter.rect_filled(band, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(229, 72, 77, 90));
                    dashed_rect(&painter, band, Color32::WHITE);
                }
                Tool::Crop => {
                    let dim = Color32::from_black_alpha(140);
                    let r = r.intersect(img_rect);
                    for piece in [
                        Rect::from_min_max(img_rect.min, pos2(img_rect.max.x, r.min.y)),
                        Rect::from_min_max(pos2(img_rect.min.x, r.max.y), img_rect.max),
                        Rect::from_min_max(pos2(img_rect.min.x, r.min.y), pos2(r.min.x, r.max.y)),
                        Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(img_rect.max.x, r.max.y)),
                    ] {
                        if piece.is_positive() {
                            painter.rect_filled(piece, CornerRadius::ZERO, dim);
                        }
                    }
                    dashed_rect(&painter, r, Color32::WHITE);
                    let (w, h) = ((r.width() / scale).round(), (r.height() / scale).round());
                    painter.text(pos2(r.min.x, r.min.y - 4.0), Align2::LEFT_BOTTOM, format!("{w} x {h}"), FontId::proportional(13.0), Color32::WHITE);
                }
                _ => {}
            }
        }
        self.text_overlay(ui, &to_scr, scale);
    }

    fn start_text(&mut self, ci: usize, p: Pos2, existing: Option<usize>) {
        self.start_text_kind(ci, p, existing, TextKind::Text);
    }

    fn start_text_kind(&mut self, ci: usize, p: Pos2, existing: Option<usize>, kind: TextKind) {
        if let Some(i) = existing {
            match &self.entries[ci].doc.shapes[i] {
                Shape::Text { pos, text, color, size, bg, shadow, font, bold, italic, .. } => {
                    self.text_edit = Some(TextEditing {
                        kind: TextKind::Text,
                        pos: *pos,
                        text: text.clone(),
                        index: Some(i),
                        color: *color,
                        size: *size,
                        bg: *bg,
                        shadow: *shadow,
                        font: font.clone(),
                        bold: *bold,
                        italic: *italic,
                        focused: false,
                    });
                    return;
                }
                Shape::Callout { pos, tail, text, color, size, shadow, font, bold, italic, .. } => {
                    self.text_edit = Some(TextEditing {
                        kind: TextKind::Callout { tail: *tail },
                        pos: *pos,
                        text: text.clone(),
                        index: Some(i),
                        color: *color,
                        size: *size,
                        bg: true,
                        shadow: *shadow,
                        font: font.clone(),
                        bold: *bold,
                        italic: *italic,
                        focused: false,
                    });
                    return;
                }
                _ => {}
            }
        }
        self.text_edit = Some(TextEditing {
            kind,
            pos: p,
            text: String::new(),
            index: None,
            color: self.color(),
            size: doc::text_size(self.settings.width),
            bg: self.settings.text_background || matches!(kind, TextKind::Callout { .. }),
            shadow: self.settings.shadow,
            font: self.settings.font.clone(),
            bold: self.bold,
            italic: self.italic,
            focused: false,
        });
    }

    fn text_overlay(&mut self, ui: &mut egui::Ui, to_scr: &dyn Fn(Pos2) -> Pos2, scale: f32) {
        let Some(t) = &self.text_edit else { return };
        let ctx = ui.ctx().clone();
        let family = self.ui_fonts.family(&ctx, &t.font.clone(), t.bold, t.italic);
        let Some(t) = &mut self.text_edit else { return };
        let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        let escape = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let at = to_scr(t.pos);
        if let TextKind::Callout { tail } = t.kind {
            ui.painter().line_segment([at, to_scr(tail)], Stroke::new(2.0, t.color));
        }
        let font = FontId::new((t.size * scale).max(6.0), family);
        let color = if t.bg { render::halo_color(t.color) } else { t.color };
        let fill = if t.bg { t.color } else { Color32::from_black_alpha(40) };
        let resp = egui::Area::new(egui::Id::new("text-edit"))
            .fixed_pos(at - vec2(4.0, 4.0))
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, ACCENT))
                    .fill(fill)
                    .inner_margin(egui::Margin::same(3))
                    .show(ui, |ui| {
                        let lines = t.text.split('\n').count().max(1);
                        let w = (doc::text_extent(&t.text, t.size).x * scale + 40.0).max(160.0);
                        ui.add(
                            egui::TextEdit::multiline(&mut t.text)
                                .font(font)
                                .text_color(color)
                                .frame(egui::Frame::NONE)
                                .desired_rows(lines)
                                .desired_width(w)
                                .hint_text("Type here"),
                        )
                    })
                    .inner
            })
            .inner;
        if !t.focused {
            resp.request_focus();
            t.focused = true;
        }
        if escape {
            self.text_edit = None;
        } else if enter {
            self.commit_text();
        }
    }

    fn mini_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let secs = self.mini_since.elapsed().as_secs();
        ui.horizontal_centered(|ui| {
            ui.add_space(10.0);
            let blink = (self.mini_since.elapsed().as_millis() / 500) % 2 == 0;
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 6.0, if blink { Color32::from_rgb(229, 72, 77) } else { Color32::from_rgb(120, 40, 44) });
            let text = match self.mini {
                Some(Mini::Gif) => format!("Recording GIF  {}:{:02}", secs / 60, secs % 60),
                Some(Mini::Steps) => format!("Recording steps: {} so far", self.guide.len()),
                Some(Mini::Scroll) if self.scroll_manual => "Scroll it yourself, then press Stop".to_string(),
                Some(Mini::Scroll) => "Scrolling and stitching...".to_string(),
                None => String::new(),
            };
            ui.label(RichText::new(text).color(Color32::WHITE).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let stop = egui::Button::new(RichText::new("Stop").color(Color32::WHITE).strong()).fill(Color32::from_rgb(229, 72, 77));
                if ui.add(stop).clicked() {
                    match self.mini {
                        Some(Mini::Steps) => self.stop_steps(&ctx),
                        _ => self.stop_job(),
                    }
                }
            });
        });
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.screenshot else { return };
        job.frames += 1;
        ctx.request_repaint();
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        if job.frames >= SCREENSHOT_FRAMES && !job.requested {
            job.requested = true;
            let slot = job.result.clone();
            ctx.request_screenshot(move |img| {
                *slot.lock().unwrap() = Some(img);
            });
        }
        let got = job.result.lock().unwrap().take();
        if let Some(img) = got {
            let [w, h] = img.size;
            let mut buf = Vec::with_capacity(w * h * 4);
            for c in &img.pixels {
                buf.extend_from_slice(&c.to_srgba_unmultiplied());
            }
            if let Some(out) = RgbaImage::from_raw(w as u32, h as u32, buf) {
                if let Some(dir) = job.path.parent()
                    && !dir.as_os_str().is_empty()
                {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = out.save(&job.path) {
                    eprintln!("couldn't save screenshot: {e}");
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if job.frames > SCREENSHOT_FRAMES + 600 {
            eprintln!("the window picture never arrived");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for SnagApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.startup_done {
            self.startup_done = true;
            if self.persist {
                match self.settings.start_mode {
                    crate::settings::StartMode::Editor => {}
                    crate::settings::StartMode::Panel => self.enter_panel(ctx),
                    crate::settings::StartMode::Tray => {
                        if self.tray.is_some() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        }
                    }
                }
            }
        }
        self.poll_hotkeys(ctx);
        self.poll_tray(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self.tray.is_some() && self.settings.close_to_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.panel_open {
                self.leave_panel(ctx);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.status = "Snagster keeps running in the corner of the taskbar. Right-click its icon to quit.".into();
        }
        self.poll_capture(ctx);
        self.poll_jobs(ctx);
        self.poll_timers(ctx);
        self.screenshot_tick(ctx);
        self.ui_test_tick(ctx);
        if self.capturing.is_some() || self.mini.is_some() || self.ocr_busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // Save settings once they have stopped changing for half a second (not every frame
        // while a slider is dragged).
        if self.persist && self.settings != self.saved_settings {
            let since = *self.settings_changed_at.get_or_insert_with(Instant::now);
            if since.elapsed() < Duration::from_millis(500) {
                ctx.request_repaint_after(Duration::from_millis(520));
                return;
            }
            self.settings_changed_at = None;
            self.settings.save();
            self.saved_settings = self.settings.clone();
            self.apply_hotkeys(ctx);
            self.apply_tray(ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.overlay.is_some() {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.handle_overlay(ui));
            return;
        }
        if self.panel_open {
            egui::CentralPanel::default().frame(Self::panel_frame()).show(ui, |ui| self.panel_ui(ui));
            self.settings_window(&ctx);
            return;
        }
        if self.mini.is_some() {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(BG_DARK).stroke(Stroke::new(1.0, ACCENT)))
                .show(ui, |ui| self.mini_bar(ui));
            return;
        }
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            self.open_path(&p);
        }
        self.shortcuts(&ctx);

        egui::Panel::top("menubar")
            .frame(egui::Frame::new().fill(BG_DARK).inner_margin(egui::Margin::symmetric(8, 3)))
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("top")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(6, 0)))
            .show(ui, |ui| self.top_bar(ui));
        if self.settings.show_tray {
            egui::Panel::bottom("tray")
                .exact_size(112.0)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(4, 6)))
                .show(ui, |ui| self.tray(ui));
        }
        egui::Panel::left("tools")
            .exact_size(118.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(4, 0)))
            .show(ui, |ui| self.tool_bar(ui));
        if self.settings.show_side_panel {
            egui::Panel::right("props")
                .exact_size(230.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 6)))
                .show(ui, |ui| self.side_panel(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG_DARK)).show(ui, |ui| self.canvas(ui));

        self.dialog_window(&ctx);
        self.settings_window(&ctx);
        self.help_windows(&ctx);
        self.library_window(&ctx);
        self.gif_window(&ctx);
        self.timer_window(&ctx);
        self.text_window(&ctx);
        self.show_pins(&ctx);
    }
}

// ---------- drawing helpers ----------

trait LabelIfSet {
    fn label_if_set(&self) -> String;
}

impl LabelIfSet for crate::settings::Combo {
    fn label_if_set(&self) -> String {
        if self.is_set() { self.label() } else { String::new() }
    }
}

pub(crate) fn item(ui: &mut egui::Ui, label: &str, keys: &str) -> bool {
    item_on(ui, true, label, keys)
}

pub(crate) fn item_on(ui: &mut egui::Ui, enabled: bool, label: &str, keys: &str) -> bool {
    let b = egui::Button::new(label).shortcut_text(keys);
    let clicked = ui.add_enabled(enabled, b).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG_DARK;
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    v.widgets.inactive.weak_bg_fill = PANEL_HI;
    v.widgets.inactive.bg_fill = PANEL_HI;
    v.widgets.inactive.corner_radius = CornerRadius::same(6);
    v.widgets.hovered.corner_radius = CornerRadius::same(6);
    v.widgets.active.corner_radius = CornerRadius::same(6);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(60, 64, 78);
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(9.0, 5.0);
    });
}

pub(crate) fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).strong().color(Color32::WHITE));
    ui.add_space(2.0);
}

fn dashed_rect(p: &egui::Painter, r: Rect, c: Color32) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    p.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.5, c), 6.0, 4.0));
}

/// A mouse-pointer picture for stamps and step guides.
fn pointer_image(px: u32) -> RgbaImage {
    let mut img = RgbaImage::new(px, px);
    let scale = px as f32 / 21.0;
    let Some(mut pm) = render::to_pixmap(&img) else { return img };
    let pts = doc::pointer_outline(pos2(1.0, 1.0), 20.0 * scale * 0.95);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(pts[0].x, pts[0].y);
    for p in &pts[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        let mut white = tiny_skia::Paint::default();
        white.set_color_rgba8(255, 255, 255, 255);
        white.anti_alias = true;
        let mut black = tiny_skia::Paint::default();
        black.set_color_rgba8(20, 20, 20, 255);
        black.anti_alias = true;
        let id = tiny_skia::Transform::identity();
        pm.fill_path(&path, &white, tiny_skia::FillRule::Winding, id, None);
        pm.stroke_path(&path, &black, &tiny_skia::Stroke { width: (scale * 1.1).max(1.0), ..Default::default() }, id, None);
    }
    img = render::from_pixmap(pm);
    img
}

/// What drawing marks on screen needs: loaded fonts, stamp pictures, the picture itself.
pub(crate) struct PaintCtx {
    families: HashMap<String, FontFamily>,
    stamps: HashMap<char, TextureId>,
    pic: Option<(TextureId, Vec2)>,
    images: HashMap<usize, TextureId>,
}

impl PaintCtx {
    fn font(&self, font: &str, bold: bool, italic: bool, size: f32) -> FontId {
        let fam = self.families.get(&fonts::key(font, bold, italic)).cloned().unwrap_or(FontFamily::Proportional);
        FontId::new(size.max(1.0), fam)
    }
}

/// Text lined up left (0), centred (1) or right (2), with an optional outline.
fn aligned_text(p: &egui::Painter, pos: Pos2, text: &str, font: FontId, color: Color32, halo: Option<Color32>, align: u8) {
    let lines: Vec<&str> = text.split('\n').collect();
    let sizes: Vec<Vec2> = lines.iter().map(|l| p.layout_no_wrap(l.to_string(), font.clone(), color).size()).collect();
    let block = sizes.iter().map(|s| s.x).fold(0.0, f32::max);
    let mut y = pos.y;
    for (l, s) in lines.iter().zip(&sizes) {
        let dx = match align {
            1 => (block - s.x) / 2.0,
            2 => block - s.x,
            _ => 0.0,
        };
        halo_text(p, pos2(pos.x + dx, y), Align2::LEFT_TOP, l, font.clone(), color, halo);
        y += s.y.max(font.size);
    }
}

fn halo_text(p: &egui::Painter, pos: Pos2, align: Align2, text: &str, font: FontId, color: Color32, halo: Option<Color32>) {
    if let Some(h) = halo {
        let r = (font.size / 14.0).max(1.0);
        for k in 0..12 {
            let a = k as f32 / 12.0 * std::f32::consts::TAU;
            p.text(pos + vec2(a.cos(), a.sin()) * r, align, text, font.clone(), h);
        }
    }
    p.text(pos, align, text, font, color);
}

/// Draw one mark on screen. `bounds` is where the picture is (for the spotlight).
pub(crate) fn paint_shape(p: &egui::Painter, s: &Shape, to: &dyn Fn(Pos2) -> Pos2, scale: f32, bounds: Rect, pc: &PaintCtx) {
    if s.shadow() {
        let mut sh = s.clone();
        sh.translate(s.shadow_offset());
        paint_one(p, &sh, to, scale, bounds, true, pc);
    }
    paint_one(p, s, to, scale, bounds, false, pc);
}

fn paint_one(p: &egui::Painter, s: &Shape, to: &dyn Fn(Pos2) -> Pos2, scale: f32, bounds: Rect, as_shadow: bool, pc: &PaintCtx) {
    let col = |c: Color32| if as_shadow { doc::shadow_color() } else { c };
    let st = |w: f32, c: Color32| Stroke::new((w * scale).max(1.0), col(c));
    let full_uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    match s {
        Shape::Arrow { a, b, color, width, heads, .. } => {
            let head = doc::arrow_head(*a, *b, *width);
            let back = doc::arrow_head(*b, *a, *width);
            let end = if *heads >= 1 { head[1] + (head[2] - head[1]) * 0.5 } else { *b };
            let start = if *heads >= 2 { back[1] + (back[2] - back[1]) * 0.5 } else { *a };
            p.line_segment([to(start), to(end)], st(*width, *color));
            for q in [start, end] {
                p.circle_filled(to(q), (width * scale * 0.5).max(0.5), col(*color));
            }
            if *heads >= 1 {
                p.add(egui::Shape::convex_polygon(head.iter().map(|q| to(*q)).collect(), col(*color), Stroke::NONE));
            }
            if *heads >= 2 {
                p.add(egui::Shape::convex_polygon(back.iter().map(|q| to(*q)).collect(), col(*color), Stroke::NONE));
            }
        }
        Shape::Line { a, b, color, width, .. } => {
            p.line_segment([to(*a), to(*b)], st(*width, *color));
            p.circle_filled(to(*a), (width * scale * 0.5).max(0.5), col(*color));
            p.circle_filled(to(*b), (width * scale * 0.5).max(0.5), col(*color));
        }
        Shape::Rect { rect, color, width, fill, radius, .. } => {
            let r = Rect::from_two_pos(to(rect.min), to(rect.max));
            let cr = CornerRadius::same((radius * scale).clamp(0.0, 255.0) as u8);
            if *fill {
                p.rect_filled(r, cr, col(*color));
            } else {
                p.rect_stroke(r, cr, st(*width, *color), StrokeKind::Middle);
            }
        }
        Shape::Ellipse { rect, color, width, fill, .. } => {
            let r = Rect::from_two_pos(to(rect.min), to(rect.max));
            if *fill {
                p.add(egui::Shape::ellipse_filled(r.center(), r.size() / 2.0, col(*color)));
            } else {
                p.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, st(*width, *color)));
            }
        }
        Shape::Pen { points, color, width, .. } => {
            let pts: Vec<Pos2> = points.iter().map(|q| to(*q)).collect();
            let r = (width * scale * 0.5).max(0.5);
            if pts.len() == 1 {
                p.circle_filled(pts[0], r, col(*color));
            } else {
                for q in [pts[0], *pts.last().unwrap()] {
                    p.circle_filled(q, r, col(*color));
                }
                p.add(egui::Shape::line(pts, st(*width, *color)));
            }
        }
        Shape::Highlight { points, color, width } => {
            let pts: Vec<Pos2> = points.iter().map(|q| to(*q)).collect();
            let c = doc::highlight_color(*color);
            if pts.len() >= 2 {
                p.add(egui::Shape::line(pts, Stroke::new((width * doc::HIGHLIGHT_WIDTH_FACTOR * scale).max(1.0), c)));
            } else if let Some(q) = pts.first() {
                let half = width * doc::HIGHLIGHT_WIDTH_FACTOR * scale * 0.5;
                p.rect_filled(Rect::from_center_size(*q, Vec2::splat(half * 2.0)), CornerRadius::ZERO, c);
            }
        }
        Shape::Text { pos, text, color, size, bg, font, bold, italic, outline, align, .. } => {
            let f = pc.font(font, *bold, *italic, size * scale);
            if *bg {
                let galley = p.layout_no_wrap(text.clone(), f.clone(), Color32::WHITE);
                let pad = size * doc::TEXT_PAD_FACTOR * scale;
                let r = Rect::from_min_size(to(*pos), galley.size()).expand(pad);
                p.rect_filled(r, CornerRadius::same((size * 0.25 * scale).clamp(0.0, 255.0) as u8), col(*color));
                if !as_shadow {
                    aligned_text(p, to(*pos), text, f, render::halo_color(*color), None, *align);
                }
            } else if as_shadow {
                aligned_text(p, to(*pos), text, f, doc::shadow_color(), None, *align);
            } else {
                let halo = if *outline { Some(render::halo_color(*color)) } else { None };
                aligned_text(p, to(*pos), text, f, *color, halo, *align);
            }
        }
        Shape::Callout { pos, tail, text, color, size, font, bold, italic, shape, outline, align, .. } => {
            let f = pc.font(font, *bold, *italic, size * scale);
            let galley = p.layout_no_wrap(if text.is_empty() { " ".into() } else { text.clone() }, f.clone(), Color32::WHITE);
            let bubble_img = Rect::from_min_size(*pos, galley.size() / scale.max(0.001)).expand(size * 0.5);
            let tri = doc::callout_tail(bubble_img, *tail);
            let r = Rect::from_two_pos(to(bubble_img.min), to(bubble_img.max));
            let edge = {
                let e = color.gamma_multiply(0.55);
                Stroke::new(((size / 12.0).max(1.5) * scale).max(1.0), Color32::from_rgb(e.r(), e.g(), e.b()))
            };
            let line = if *outline && !as_shadow { edge } else { Stroke::NONE };
            p.add(egui::Shape::convex_polygon(tri.iter().map(|q| to(*q)).collect(), col(*color), line));
            match shape {
                crate::settings::CalloutShape::Oval => {
                    let o = r.expand2(vec2(r.width() * 0.12, r.height() * 0.18));
                    p.add(egui::Shape::ellipse_filled(o.center(), o.size() / 2.0, col(*color)));
                    if line != Stroke::NONE {
                        p.add(egui::Shape::ellipse_stroke(o.center(), o.size() / 2.0, line));
                    }
                }
                _ => {
                    let cr = if *shape == crate::settings::CalloutShape::Rectangle { 0.0 } else { size * 0.45 * scale };
                    let cr = CornerRadius::same(cr.clamp(0.0, 255.0) as u8);
                    p.rect_filled(r, cr, col(*color));
                    if line != Stroke::NONE {
                        p.rect_stroke(r, cr, line, StrokeKind::Middle);
                    }
                }
            }
            if !as_shadow {
                aligned_text(p, to(*pos), text, f, render::halo_color(*color), None, *align);
            }
        }
        Shape::Image { rect, image } => {
            if let Some(t) = pc.images.get(&(Arc::as_ptr(&image.0) as usize)) {
                let r = Rect::from_two_pos(to(rect.min), to(rect.max));
                let tint = if as_shadow { Color32::from_black_alpha(90) } else { Color32::WHITE };
                p.image(*t, r, full_uv, tint);
            }
        }
        Shape::Step { center, n, color, radius, style, .. } => {
            let c = to(*center);
            let r = radius * scale;
            p.circle_filled(c, r, col(*color));
            if !as_shadow {
                p.circle_stroke(c, r, Stroke::new(((radius / 8.0).max(2.0) * scale).max(1.0), Color32::WHITE));
                let label = style.format(*n);
                let shrink = if label.chars().count() > 2 { 0.75 } else { 1.0 };
                p.text(c, Align2::CENTER_CENTER, label, FontId::proportional((r * 1.15 * shrink).max(1.0)), render::halo_color(*color));
            }
        }
        Shape::Stamp { center, emoji, size, .. } => {
            if let Some(t) = emoji.chars().next().and_then(|c| pc.stamps.get(&c)) {
                p.image(*t, Rect::from_center_size(to(*center), Vec2::splat(size * scale)), full_uv, Color32::WHITE);
            }
        }
        Shape::Pointer { pos, size } => {
            if let Some(t) = pc.stamps.get(&POINTER_KEY) {
                let k = size * scale * 21.0 / 20.0 / 0.95;
                let tint = if as_shadow { Color32::from_black_alpha(100) } else { Color32::WHITE };
                p.image(*t, Rect::from_min_size(to(*pos) - Vec2::splat(k / 21.0), Vec2::splat(k)), full_uv, tint);
            }
        }
        Shape::Magnify { center, radius, zoom } => {
            let c = to(*center);
            let r = radius * scale;
            if let Some((tex, size)) = pc.pic {
                let uvc = pos2(center.x / size.x, center.y / size.y);
                let uvr = vec2(radius / zoom.max(1.0) / size.x, radius / zoom.max(1.0) / size.y);
                let mut mesh = egui::epaint::Mesh::with_texture(tex);
                mesh.colored_vertex(c, Color32::WHITE);
                mesh.vertices[0].uv = uvc;
                let n = 64;
                for i in 0..=n {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    let d = vec2(a.cos(), a.sin());
                    let idx = mesh.vertices.len() as u32;
                    mesh.vertices.push(egui::epaint::Vertex { pos: c + d * r, uv: uvc + vec2(d.x * uvr.x, d.y * uvr.y), color: Color32::WHITE });
                    if i > 0 {
                        mesh.add_triangle(0, idx - 1, idx);
                    }
                }
                p.add(egui::Shape::mesh(mesh));
            }
            let ring = (radius / 30.0).max(2.0) * scale;
            p.circle_stroke(c, r, Stroke::new(ring + 2.0 * scale, Color32::from_rgb(40, 42, 50)));
            p.circle_stroke(c, r, Stroke::new(ring, Color32::WHITE));
        }
        Shape::Spotlight { rect } => {
            let h = Rect::from_two_pos(to(rect.min), to(rect.max)).intersect(bounds);
            let dim = Color32::from_black_alpha(doc::SPOTLIGHT_ALPHA);
            for piece in [
                Rect::from_min_max(bounds.min, pos2(bounds.max.x, h.min.y)),
                Rect::from_min_max(pos2(bounds.min.x, h.max.y), bounds.max),
                Rect::from_min_max(pos2(bounds.min.x, h.min.y), pos2(h.min.x, h.max.y)),
                Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(bounds.max.x, h.max.y)),
            ] {
                if piece.is_positive() {
                    p.rect_filled(piece, CornerRadius::ZERO, dim);
                }
            }
        }
    }
}

pub(crate) fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    logo_at(ui.painter(), rect.center(), size);
}

/// Snagster's mark: a rounded tile with viewfinder corners and a dot.
pub fn logo_at(p: &egui::Painter, c: Pos2, size: f32) {
    let r = Rect::from_center_size(c, Vec2::splat(size));
    p.rect_filled(r, CornerRadius::same((size * 0.25) as u8), ACCENT);
    frame_icon(p, r.shrink(size * 0.2), Color32::WHITE, false);
    p.circle_filled(c, size * 0.1, Color32::WHITE);
}

fn frame_icon(p: &egui::Painter, r: Rect, c: Color32, filled_middle: bool) {
    let l = r.width() * 0.32;
    let s = Stroke::new((r.width() / 12.0).max(1.5), c);
    for (corner, dx, dy) in [(r.left_top(), 1.0, 1.0), (r.right_top(), -1.0, 1.0), (r.left_bottom(), 1.0, -1.0), (r.right_bottom(), -1.0, -1.0)] {
        p.line_segment([corner, corner + vec2(l * dx, 0.0)], s);
        p.line_segment([corner, corner + vec2(0.0, l * dy)], s);
    }
    if filled_middle {
        p.rect_filled(r.shrink(r.width() * 0.3), CornerRadius::same(2), c.gamma_multiply(0.5));
    }
}

fn folder_icon(p: &egui::Painter, r: Rect, c: Color32) {
    let s = Stroke::new(2.0, c);
    let body = Rect::from_min_max(pos2(r.left(), r.top() + r.height() * 0.25), r.max);
    p.rect_stroke(body, CornerRadius::same(3), s, StrokeKind::Middle);
    p.line_segment([pos2(r.left() + 2.0, r.top() + r.height() * 0.25), pos2(r.left() + 2.0, r.top() + 3.0)], s);
    p.line_segment([pos2(r.left() + 2.0, r.top() + 3.0), pos2(r.left() + r.width() * 0.4, r.top() + 3.0)], s);
}

fn tool_icon(p: &egui::Painter, r: Rect, t: Tool, fg: Color32, accent: Color32) {
    let s = Stroke::new(2.0, fg);
    let (l, rt, tp, b) = (r.left(), r.right(), r.top(), r.bottom());
    match t {
        Tool::Select => {
            let pts = vec![
                pos2(l + 5.0, tp + 1.0),
                pos2(l + 5.0, b - 3.0),
                pos2(l + 9.0, b - 7.0),
                pos2(l + 12.5, b),
                pos2(l + 15.0, b - 1.5),
                pos2(l + 11.5, b - 8.5),
                pos2(rt - 3.0, b - 8.5),
            ];
            p.add(egui::Shape::closed_line(pts, s));
        }
        Tool::Arrow => {
            let (a, bb) = (pos2(l + 2.0, b - 2.0), pos2(rt - 2.0, tp + 2.0));
            let head = doc::arrow_head(a, bb, 2.2);
            let h: Vec<Pos2> = head.iter().map(|q| bb + (*q - bb) * 0.55).collect();
            p.line_segment([a, h[1] + (h[2] - h[1]) * 0.5], s);
            p.add(egui::Shape::convex_polygon(h, fg, Stroke::NONE));
        }
        Tool::Line => {
            p.line_segment([pos2(l + 2.0, b - 2.0), pos2(rt - 2.0, tp + 2.0)], s);
        }
        Tool::Rect => {
            p.rect_stroke(r.shrink(3.0), CornerRadius::ZERO, s, StrokeKind::Middle);
        }
        Tool::Ellipse => {
            p.add(egui::Shape::ellipse_stroke(r.center(), vec2(r.width() / 2.0 - 1.0, r.height() / 2.0 - 4.0), s));
        }
        Tool::Pen => {
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| pos2(l + 2.0 + i as f32 * (r.width() - 4.0) / 12.0, r.center().y + (i as f32 * 0.9).sin() * 5.0))
                .collect();
            p.add(egui::Shape::line(pts, s));
        }
        Tool::Highlight => {
            p.rect_filled(
                Rect::from_min_max(pos2(l, r.center().y - 4.0), pos2(rt, r.center().y + 6.0)),
                CornerRadius::ZERO,
                doc::highlight_color(Color32::from_rgb(255, 197, 61)),
            );
            p.line_segment([pos2(l + 3.0, r.center().y + 1.0), pos2(rt - 3.0, r.center().y + 1.0)], Stroke::new(1.5, fg));
        }
        Tool::Text => {
            p.line_segment([pos2(l + 3.0, tp + 3.0), pos2(rt - 3.0, tp + 3.0)], Stroke::new(2.5, fg));
            p.line_segment([pos2(r.center().x, tp + 3.0), pos2(r.center().x, b - 2.0)], Stroke::new(2.5, fg));
        }
        Tool::Callout => {
            let bubble = Rect::from_min_max(pos2(l, tp + 1.0), pos2(rt, b - 6.0));
            p.rect_stroke(bubble, CornerRadius::same(5), s, StrokeKind::Middle);
            p.line_segment([pos2(l + 5.0, b - 6.0), pos2(l + 3.0, b)], s);
            p.line_segment([pos2(l + 3.0, b), pos2(l + 10.0, b - 6.0)], s);
        }
        Tool::Step => {
            p.circle_filled(r.center(), r.width() / 2.0 - 1.0, accent);
            p.text(r.center(), Align2::CENTER_CENTER, "1", FontId::proportional(12.0), render::halo_color(accent));
        }
        Tool::Stamp => {
            p.circle_stroke(r.center(), r.width() / 2.0 - 1.0, s);
            p.circle_filled(r.center() + vec2(-3.5, -2.5), 1.6, fg);
            p.circle_filled(r.center() + vec2(3.5, -2.5), 1.6, fg);
            let smile: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.15 + 0.7 * i as f32 / 8.0);
                    r.center() + vec2(a.cos() * 5.5, a.sin() * 5.5 - 0.5)
                })
                .collect();
            p.add(egui::Shape::line(smile, Stroke::new(1.6, fg)));
        }
        Tool::Magnify => {
            p.circle_stroke(r.center() - vec2(2.0, 2.0), 6.5, s);
            p.line_segment([r.center() + vec2(2.8, 2.8), r.right_bottom() - vec2(1.0, 1.0)], Stroke::new(3.0, fg));
        }
        Tool::Spotlight => {
            p.rect_filled(r, CornerRadius::same(2), Color32::from_gray(70));
            p.rect_filled(r.shrink(5.0), CornerRadius::same(2), Color32::from_rgb(255, 230, 150));
            p.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, fg), StrokeKind::Inside);
        }
        Tool::Blur => {
            let n = 4;
            let cell = r.width() / n as f32;
            for y in 0..n {
                for x in 0..n {
                    let g = 90 + ((x * 37 + y * 53) % 5) as u8 * 30;
                    let c = Rect::from_min_size(pos2(l + x as f32 * cell, tp + y as f32 * cell), Vec2::splat(cell - 1.0));
                    p.rect_filled(c, CornerRadius::ZERO, Color32::from_gray(g));
                }
            }
        }
        Tool::Selection => {
            p.extend(egui::Shape::dashed_line(&[r.left_top() + vec2(2.0, 2.0), r.right_top() + vec2(-2.0, 2.0), r.right_bottom() - vec2(2.0, 2.0), r.left_bottom() + vec2(2.0, -2.0), r.left_top() + vec2(2.0, 2.0)], Stroke::new(1.8, fg), 3.0, 2.0));
        }
        Tool::Fill => {
            let pts = vec![pos2(l + 3.0, tp + 8.0), pos2(l + 10.0, tp + 2.0), pos2(rt - 4.0, tp + 10.0), pos2(l + 10.0, b - 3.0)];
            p.add(egui::Shape::closed_line(pts, s));
            p.circle_filled(pos2(rt - 3.0, b - 4.0), 3.0, accent);
        }
        Tool::Eraser => {
            let c = r.center();
            let d = vec2(0.7, -0.7);
            let n = vec2(0.7, 0.7);
            let pts = vec![c - d * 8.0 - n * 4.0, c + d * 8.0 - n * 4.0, c + d * 8.0 + n * 4.0, c - d * 8.0 + n * 4.0];
            p.add(egui::Shape::closed_line(pts, s));
            p.line_segment([c - n * 4.0, c + n * 4.0], s);
        }
        Tool::CutOut => {
            p.rect_stroke(Rect::from_min_max(pos2(l + 1.0, tp + 1.0), pos2(rt - 1.0, tp + 7.0)), CornerRadius::ZERO, s, StrokeKind::Middle);
            p.rect_stroke(Rect::from_min_max(pos2(l + 1.0, b - 7.0), pos2(rt - 1.0, b - 1.0)), CornerRadius::ZERO, s, StrokeKind::Middle);
            p.extend(egui::Shape::dashed_line(&[pos2(l, r.center().y), pos2(rt, r.center().y)], Stroke::new(1.5, Color32::from_rgb(229, 72, 77)), 3.0, 2.0));
        }
        Tool::Crop => {
            p.line_segment([pos2(l + 6.0, tp), pos2(l + 6.0, b - 6.0)], s);
            p.line_segment([pos2(l + 6.0, b - 6.0), pos2(rt, b - 6.0)], s);
            p.line_segment([pos2(l, tp + 6.0), pos2(rt - 6.0, tp + 6.0)], s);
            p.line_segment([pos2(rt - 6.0, tp + 6.0), pos2(rt - 6.0, b)], s);
        }
    }
}

#[allow(dead_code)]
fn shot_title(s: &Shot) -> &str {
    &s.title
}

// ---------- tray icon and capture panel ----------

impl SnagApp {
    /// Show (or rebuild, when presets or hotkeys changed) the icon in the taskbar corner.
    pub(crate) fn apply_tray(&mut self, ctx: &egui::Context) {
        if !self.persist {
            return;
        }
        if !self.settings.tray_icon {
            self.tray = None;
            self.tray_for = None;
            return;
        }
        let key = (
            self.settings.presets.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            self.settings.bindings.iter().map(|b| b.combo.label()).collect::<Vec<_>>(),
        );
        if self.tray.is_some() && self.tray_for.as_ref() == Some(&key) {
            return;
        }
        self.tray = None;
        let ctx2 = ctx.clone();
        match crate::tray::create(&self.settings, self.tray_tx.clone(), move || ctx2.request_repaint()) {
            Ok(t) => {
                self.tray = Some(t);
                self.tray_for = Some(key);
            }
            Err(e) => self.status = format!("The taskbar-corner icon couldn't be added: {e}"),
        }
    }

    fn poll_tray(&mut self, ctx: &egui::Context) {
        use crate::tray::TrayCmd;
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCmd::Action(a) => {
                    if self.panel_open {
                        self.leave_panel(ctx);
                    }
                    self.do_action(ctx, a);
                }
                TrayCmd::Preset(i) => {
                    if self.panel_open {
                        self.leave_panel(ctx);
                    }
                    self.run_preset(ctx, i);
                }
                TrayCmd::TogglePanel => {
                    if self.panel_open {
                        self.hide_panel(ctx);
                    } else {
                        self.enter_panel(ctx);
                    }
                }
                TrayCmd::Panel => self.enter_panel(ctx),
                TrayCmd::Editor => {
                    if self.panel_open {
                        self.leave_panel(ctx);
                    }
                    self.show_window(ctx);
                }
                TrayCmd::Library => {
                    if self.panel_open {
                        self.leave_panel(ctx);
                    }
                    self.library_open = true;
                    self.show_window(ctx);
                }
                TrayCmd::Settings => {
                    if self.panel_open {
                        self.leave_panel(ctx);
                    }
                    self.settings_open = true;
                    self.show_window(ctx);
                }
                TrayCmd::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    /// Turn the window into the compact capture panel, just above the taskbar corner.
    pub(crate) fn enter_panel(&mut self, ctx: &egui::Context) {
        if self.panel_open || self.mini.is_some() || self.overlay.is_some() {
            if self.panel_open {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            return;
        }
        self.commit_text();
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        if let (Some(o), Some(i)) = (outer, inner) {
            self.panel_restore = Some((pos2(o.min.x * ppp, o.min.y * ppp), i.size()));
        }
        let size = crate::panel::PANEL_SIZE;
        let at = capture::cursor_pos().unwrap_or((0, 0));
        let mons = capture::monitors();
        let m = mons
            .iter()
            .find(|m| at.0 >= m.x && at.1 >= m.y && at.0 < m.x + m.width as i32 && at.1 < m.y + m.height as i32)
            .or_else(|| mons.iter().find(|m| m.primary))
            .or(mons.first());
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(300.0, 150.0)));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        if let Some(m) = m {
            // Bottom-right, above the taskbar, like a pop-up from the tray icon.
            let x = (m.x + m.width as i32) as f32 - (size.x + 16.0) * ppp;
            let y = (m.y + m.height as i32) as f32 - (size.y + 64.0) * ppp;
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(x / ppp, y / ppp)));
        }
        if !UI_TEST.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.panel_open = true;
    }

    /// Back to the normal window shape (it stays hidden or shown as it was).
    pub(crate) fn leave_panel(&mut self, ctx: &egui::Context) {
        if !self.panel_open {
            return;
        }
        self.panel_open = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(MIN_SIZE));
        if !UI_TEST.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        }
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
        let (pos, size) = self.panel_restore.take().unwrap_or((pos2(120.0 * ppp, 80.0 * ppp), vec2(1280.0, 800.0)));
        let size = if size.x < 900.0 || size.y < 560.0 { vec2(1280.0, 800.0) } else { size };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(pos.x / ppp, pos.y / ppp)));
    }

    /// Close the panel: back to the taskbar corner (or the taskbar when there's no icon).
    pub(crate) fn hide_panel(&mut self, ctx: &egui::Context) {
        self.leave_panel(ctx);
        if self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    /// The capture panel's "Share" choice, done after a capture.
    fn panel_share(&mut self, share: crate::settings::PanelShare) {
        use crate::settings::PanelShare;
        match share {
            PanelShare::None => {}
            PanelShare::Clipboard => self.copy(),
            PanelShare::SaveFolder => self.quick_save(),
            PanelShare::Pin => self.pin_current(),
            PanelShare::OpenWith => self.open_externally(),
            PanelShare::Cloud => {
                if let (Some((_, folder)), Some(i)) = (crate::settings::cloud_folders().into_iter().next(), self.current) {
                    let title = self.entries[i].name.clone();
                    let base = self.settings.file_name(&title);
                    let path = crate::settings::unique_path(&folder.join("Snagster"), &base, self.settings.format.ext());
                    self.save_to(i, path);
                } else {
                    self.status = "No cloud folder (OneDrive, Dropbox, Google Drive...) was found.".into();
                }
            }
        }
    }
}

// ---------- round 5: marks clipboard, files, sharing ----------

impl SnagApp {
    pub(crate) fn remember_file(&mut self, p: &std::path::Path) {
        let s = p.display().to_string();
        self.settings.recent_files.retain(|f| *f != s);
        self.settings.recent_files.insert(0, s);
        self.settings.recent_files.truncate(12);
    }

    pub(crate) fn copy_mark(&mut self) {
        if let Some(s) = self.selected
            && let Some(i) = self.current
            && let Some(m) = self.entries[i].doc.shapes.get(s)
        {
            self.mark_clip = Some(m.clone());
            self.status = "Mark copied. Ctrl+V pastes it.".into();
        }
    }

    pub(crate) fn cut_mark(&mut self) {
        self.copy_mark();
        if self.mark_clip.is_some() {
            self.delete_selected();
        }
    }

    pub(crate) fn paste_mark(&mut self) {
        let Some(mut m) = self.mark_clip.clone() else { return };
        m.translate(vec2(24.0, 24.0));
        if let Some(e) = self.entry() {
            e.push_undo();
            e.doc.shapes.push(m.clone());
            let n = e.doc.shapes.len() - 1;
            self.selected = Some(n);
            self.tool = Tool::Select;
            self.mark_clip = Some(m);
        }
    }

    /// Put the clipboard's picture on the current picture as a movable piece.
    pub(crate) fn paste_as_piece(&mut self) {
        let got = arboard::Clipboard::new().and_then(|mut c| c.get_image());
        let Ok(img) = got else {
            self.status = "There's no picture on the clipboard.".into();
            return;
        };
        let Some(rgba) = RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()) else { return };
        let Some(e) = self.entry() else { return };
        let size = e.doc.size();
        let s = (size.x * 0.6 / rgba.width() as f32).min(size.y * 0.6 / rgba.height() as f32).min(1.0);
        let r = Rect::from_center_size(pos2(size.x / 2.0, size.y / 2.0), vec2(rgba.width() as f32 * s, rgba.height() as f32 * s));
        e.push_undo();
        e.doc.shapes.push(Shape::Image { rect: r, image: doc::ImgData(Arc::new(rgba)) });
        let n = e.doc.shapes.len() - 1;
        self.selected = Some(n);
        self.tool = Tool::Select;
        self.status = "Pasted as a piece: drag to move, [ and ] resize.".into();
    }

    /// Save where the picture came from (asks where only the first time).
    pub(crate) fn save(&mut self) {
        self.commit_text();
        let Some(i) = self.current else { return };
        match self.entries[i].path.clone() {
            Some(p) if p.parent().is_some_and(|d| d.is_dir()) => self.save_to(i, p),
            _ => self.save_as(),
        }
    }

    pub(crate) fn rename_current(&mut self, new_name: &str) {
        let Some(i) = self.current else { return };
        let name = new_name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(p) = self.entries[i].path.clone() {
            let ext = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "png".into());
            let bad = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
            let clean: String = name.chars().map(|c| if bad.contains(&c) { '_' } else { c }).collect();
            let target = p.with_file_name(format!("{clean}.{ext}"));
            match std::fs::rename(&p, &target) {
                Ok(()) => {
                    self.entries[i].path = Some(target.clone());
                    self.remember_file(&target);
                }
                Err(e) => {
                    self.status = format!("Couldn't rename the file: {e}");
                    return;
                }
            }
        }
        self.entries[i].name = name.to_string();
        self.status = format!("Renamed to {name}.");
    }

    pub(crate) fn recycle_current(&mut self) {
        let Some(i) = self.current else { return };
        if let Some(p) = self.entries[i].path.clone() {
            if let Err(e) = trash::delete(&p) {
                self.status = format!("Couldn't move it to the Recycle Bin: {e}");
                return;
            }
            let s = p.display().to_string();
            self.settings.recent_files.retain(|f| *f != s);
            self.lib_loaded = false;
            self.status = format!("Moved {} to the Recycle Bin.", p.display());
        } else {
            self.status = "Removed from Recent (it was never saved).".into();
        }
        self.remove_current();
    }

    fn temp_png(&mut self) -> Option<PathBuf> {
        self.commit_text();
        let i = self.current?;
        let base: String = self.entries[i].name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        let path = std::env::temp_dir().join(format!("{base}_{}.png", chrono::Local::now().format("%H%M%S")));
        export::save_image(&render::compose(&self.entries[i].doc), &path, 92).ok()?;
        Some(path)
    }

    pub(crate) fn print(&mut self) {
        let Some(path) = self.temp_png() else { return };
        #[cfg(windows)]
        let r = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command"])
            .arg(format!("Start-Process -FilePath '{}' -Verb Print", path.display()))
            .spawn();
        #[cfg(not(windows))]
        let r = std::process::Command::new("lp").arg(&path).spawn();
        self.status = match r {
            Ok(_) => "Sent to the printer dialog.".into(),
            Err(e) => format!("Couldn't print: {e}"),
        };
    }

    /// Start an email with the picture attached (Outlook), or open the mail program and
    /// show the file so it can be dragged in.
    pub(crate) fn email(&mut self) {
        let Some(path) = self.temp_png() else { return };
        #[cfg(windows)]
        {
            let candidates = [
                r"C:\Program Files\Microsoft Office\root\Office16\OUTLOOK.EXE",
                r"C:\Program Files (x86)\Microsoft Office\root\Office16\OUTLOOK.EXE",
                r"C:\Program Files\Microsoft Office\Office16\OUTLOOK.EXE",
            ];
            if let Some(ol) = candidates.iter().find(|c| std::path::Path::new(c).exists())
                && std::process::Command::new(ol).arg("/a").arg(&path).spawn().is_ok()
            {
                self.status = "A new Outlook email with the picture is open.".into();
                return;
            }
            let _ = std::process::Command::new("cmd").args(["/C", "start", "", "mailto:?subject=Snagster%20capture"]).spawn();
        }
        self.open_folder(&path);
        self.status = "Your mail program is open; drag the picture from the folder into the email.".into();
    }

    /// A Word document with the pictures (the recorded steps, or all of Recent), opened.
    pub(crate) fn export_word(&mut self) {
        self.commit_text();
        let items = self.guide_items();
        if items.is_empty() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Word document", &["docx"])
            .set_file_name(format!("{}.docx", self.guide_title))
            .save_file()
        else {
            return;
        };
        match export::docx(&self.guide_title, &items, &path) {
            Ok(()) => {
                self.status = format!("Saved {}", path.display());
                #[cfg(windows)]
                let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(&path).spawn();
            }
            Err(e) => self.status = format!("Couldn't make the Word document: {e}"),
        }
    }

    /// Replace the picture with `f(picture)`, marks flattened first (undo brings them back).
    pub(crate) fn image_op(&mut self, what: &str, f: impl Fn(&RgbaImage) -> RgbaImage) {
        self.commit_text();
        self.selected = None;
        let Some(e) = self.entry() else { return };
        let out = f(&render::compose(&e.doc));
        let (w, h) = out.dimensions();
        e.push_undo();
        e.doc = Doc::new(out);
        self.zoom = None;
        self.pan = Vec2::ZERO;
        self.status = format!("{what}: now {w} x {h}.");
    }

    fn template_menu(&mut self, ui: &mut egui::Ui) {
        let n = self.entries.len();
        ui.label(RichText::new(format!("Made from the {n} pictures in Recent")).color(TEXT_DIM));
        let imgs: Vec<RgbaImage> = self.entries.iter().map(|e| render::compose(&e.doc)).collect();
        let mut made: Option<(String, RgbaImage)> = None;
        if item_on(ui, n >= 1, "Step-by-step guide (one page)", "") {
            let items = self.guide_items();
            made = effects::guide_page(&self.guide_title, &items, 1200).map(|i| ("Guide".into(), i));
        }
        if item_on(ui, n >= 2, "Side by side", "") {
            made = effects::grid(&imgs, n as u32, 24, &[]).map(|i| ("Side by side".into(), i));
        }
        if item_on(ui, n >= 2, "Grid, 2 across", "") {
            made = effects::grid(&imgs, 2, 24, &[]).map(|i| ("Grid".into(), i));
        }
        if item_on(ui, n >= 3, "Grid, 3 across", "") {
            made = effects::grid(&imgs, 3, 24, &[]).map(|i| ("Grid".into(), i));
        }
        if item_on(ui, n >= 2, "Before and after (last two)", "") {
            let two = imgs[imgs.len() - 2..].to_vec();
            made = effects::grid(&two, 2, 24, &["Before", "After"]).map(|i| ("Before and after".into(), i));
        }
        if item_on(ui, n >= 2, "Numbered gallery", "") {
            let labels: Vec<String> = (1..=n).map(|i| format!("{i}.")).collect();
            let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
            made = effects::grid(&imgs, 3, 24, &refs).map(|i| ("Gallery".into(), i));
        }
        if let Some((name, img)) = made {
            let name = self.next_name(&name);
            self.add_entry(name, Doc::new(img));
            self.status = "Made from a template. Mark it up like any picture.".into();
        }
    }

    /// Per-tool options in the side panel for the new choices, and Quick Styles.
    pub(crate) fn extra_tool_options(&mut self, ui: &mut egui::Ui) {
        use crate::settings::{BlurMode, CalloutShape};
        let t = self.tool;
        if t == Tool::Arrow {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Heads").color(TEXT_DIM));
                ui.selectable_value(&mut self.settings.arrow_heads, 0, "none");
                ui.selectable_value(&mut self.settings.arrow_heads, 1, "end");
                ui.selectable_value(&mut self.settings.arrow_heads, 2, "both");
            });
        }
        if matches!(t, Tool::Text | Tool::Callout) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Align").color(TEXT_DIM));
                ui.selectable_value(&mut self.settings.text_align, 0, "left");
                ui.selectable_value(&mut self.settings.text_align, 1, "center");
                ui.selectable_value(&mut self.settings.text_align, 2, "right");
            });
        }
        if t == Tool::Text {
            ui.checkbox(&mut self.settings.text_outline, "Outline around letters");
        }
        if t == Tool::Callout {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Shape").color(TEXT_DIM));
                for (s, l) in [(CalloutShape::Rounded, "rounded"), (CalloutShape::Rectangle, "box"), (CalloutShape::Oval, "oval")] {
                    ui.selectable_value(&mut self.settings.callout_shape, s, l);
                }
            });
            ui.checkbox(&mut self.settings.callout_outline, "Darker outline");
        }
        if t == Tool::Blur {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Type").color(TEXT_DIM));
                for m in BlurMode::ALL {
                    ui.selectable_value(&mut self.settings.blur_mode, m, m.label());
                }
            });
            if ui.button("Smart redact this picture").clicked()
                && let Some(i) = self.current
            {
                let img = render::compose(&self.entries[i].doc);
                self.run_ocr(OcrPurpose::Redact(i), img);
            }
        }
        if t == Tool::Eraser {
            ui.label(RichText::new(format!("Eraser width {:.0} px", self.settings.width * 3.0)).color(TEXT_DIM));
        }
        // Quick Styles: saved looks, one click away.
        ui.add_space(8.0);
        section(ui, "Quick styles");
        let mut apply = None;
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (i, q) in self.settings.quick_styles.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                ui.painter().rect_filled(r, CornerRadius::same(6), PANEL_HI);
                let c = rgb(q.color);
                if q.fill {
                    ui.painter().circle_filled(r.center(), 9.0, c);
                } else {
                    ui.painter().circle_stroke(r.center(), 8.0, Stroke::new((q.width / 2.0).clamp(1.5, 5.0), c));
                }
                if q.shadow {
                    ui.painter().rect_stroke(r.shrink(1.0), CornerRadius::same(6), Stroke::new(1.0, Color32::from_gray(110)), StrokeKind::Inside);
                }
                if resp.clicked() {
                    apply = Some(i);
                }
                resp.on_hover_text("Click to use. Right-click to remove.").context_menu(|ui| {
                    if ui.button("Remove this style").clicked() {
                        remove = Some(i);
                        ui.close();
                    }
                });
            }
            if ui.button(" + ").on_hover_text("Save the current color, size and switches as a style").clicked() {
                let s = &self.settings;
                let q = crate::settings::QuickStyle { color: s.color, width: s.width, shadow: s.shadow, fill: s.fill, text_background: s.text_background };
                self.settings.quick_styles.push(q);
            }
        });
        if let Some(i) = apply {
            let q = self.settings.quick_styles[i].clone();
            self.set_color(rgb(q.color));
            self.settings.width = q.width;
            self.settings.shadow = q.shadow;
            self.settings.fill = q.fill;
            self.settings.text_background = q.text_background;
        }
        if let Some(i) = remove {
            self.settings.quick_styles.remove(i);
        }
    }
}
