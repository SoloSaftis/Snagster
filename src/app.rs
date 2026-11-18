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

const ACCENT_DEFAULT: Color32 = Color32::from_rgb(79, 124, 255);

/// The accent colour: the desktop's where it has one (`look::accent`), else Snagster's blue.
pub fn accent() -> Color32 {
    crate::look::system_accent().unwrap_or(ACCENT_DEFAULT)
}
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
/// What `--screenshot` shows (`--view`).
pub static SCREENSHOT_VIEW: Mutex<String> = Mutex::new(String::new());

mod uitest;
#[cfg(test)]
mod tool_tests;
pub(crate) mod access_ui;
pub(crate) mod earlier;
pub(crate) mod work;
pub(crate) mod ribbon;
pub(crate) mod studio_ui;
pub(crate) mod stream_ui;
pub(crate) mod updates_ui;
pub(crate) mod commands_ui;
#[cfg(any(target_os = "macos", test))]
pub(crate) mod menubar_ui;
/// The editor window's smallest size (lifted while it is the panel or the recording bar).
pub const MIN_SIZE: Vec2 = Vec2::new(900.0, 560.0);

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// What a background job hands back to the window's thread to finish there.
pub(crate) type Apply = Box<dyn FnOnce(&mut SnagApp, &egui::Context) + Send>;

/// A picture's texture that hasn't been drawn for this long is let go (and made again when
/// it is shown), so a long Recent list doesn't keep every full-size picture on the graphics chip.
const TEXTURE_KEEP: Duration = Duration::from_secs(90);

pub(crate) struct Entry {
    /// Stays the same while the entry lives, so background jobs find it again.
    pub id: u64,
    pub name: String,
    pub doc: Doc,
    pub undo: Vec<Doc>,
    pub redo: Vec<Doc>,
    tex: Option<(Arc<RgbaImage>, TextureHandle)>,
    drawn: Instant,
    pub path: Option<PathBuf>,
}

impl Entry {
    pub fn new(name: String, doc: Doc) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Self { id, name, doc, undo: Vec::new(), redo: Vec::new(), tex: None, drawn: Instant::now(), path: None }
    }

    /// Let go of the texture when it hasn't been drawn for `keep`.
    pub fn drop_stale_texture(&mut self, now: Instant, keep: Duration) -> bool {
        if self.tex.is_some() && work::texture_stale(self.drawn, now, keep) {
            self.tex = None;
            return true;
        }
        false
    }

    pub fn push_undo(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn texture(&mut self, ctx: &egui::Context) -> TextureId {
        self.drawn = Instant::now();
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
    /// Reshaping a mark by one of its handles; `orig` is the mark as it was when taken.
    Grip { idx: usize, grip: doc::Grip, orig: Box<Shape>, pushed: bool },
}

/// How near (in screen points) the pointer must be to take a mark's handle.
const GRIP_REACH: f32 = 8.0;

/// Tools that can take and adjust marks, not only draw new ones.
fn grabs(tool: Tool) -> bool {
    matches!(tool, Tool::Select | Tool::Arrow | Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Highlight | Tool::Spotlight | Tool::Magnify | Tool::Callout)
}

/// Whether a drawing tool owns this mark: pressing on it with that tool picks it up to move or
/// reshape it instead of drawing a new one (the selection tool owns every mark).
fn tool_owns(tool: Tool, s: &Shape) -> bool {
    match tool {
        Tool::Select => true,
        Tool::Arrow => matches!(s, Shape::Arrow { .. }),
        Tool::Line => matches!(s, Shape::Line { .. }),
        Tool::Rect => matches!(s, Shape::Rect { .. }),
        Tool::Ellipse => matches!(s, Shape::Ellipse { .. }),
        Tool::Highlight => matches!(s, Shape::Marker { .. }),
        Tool::Spotlight => matches!(s, Shape::Spotlight { .. }),
        Tool::Magnify => matches!(s, Shape::Magnify { .. }),
        Tool::Callout => matches!(s, Shape::Callout { .. }),
        _ => false,
    }
}

/// The pointer shape over a handle.
fn grip_cursor(g: doc::Grip) -> egui::CursorIcon {
    use doc::Grip;
    match g {
        Grip::Corner(0 | 2) => egui::CursorIcon::ResizeNwSe,
        Grip::Corner(_) => egui::CursorIcon::ResizeNeSw,
        Grip::Side(0 | 2) => egui::CursorIcon::ResizeVertical,
        Grip::Side(_) => egui::CursorIcon::ResizeHorizontal,
        _ => egui::CursorIcon::Grab,
    }
}

/// Take a press of Enter that finishes typing. Shift+Enter is left for the text box, which
/// starts a new line with it (egui's `consume_key` ignores Shift, so it can't tell them apart).
fn take_finishing_enter(events: &mut Vec<egui::Event>) -> bool {
    let mut hit = false;
    events.retain(|e| {
        let m = matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift);
        hit |= m;
        !m
    });
    hit
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
    Language,
    Streaming,
}

/// The small always-on-top bar shown while something runs in the background.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mini {
    Gif,
    Video,
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
    /// The frames still coming from the thread that reads the GIF.
    pub loading: Option<Receiver<GifMsg>>,
}

/// What the thread reading a GIF sends.
pub(crate) enum GifMsg {
    Frame(RgbaImage, u32),
    Done(Result<(), String>),
}

/// How many GIF frames become textures per frame of the window (the rest wait).
const GIF_FRAMES_PER_PAINT: usize = 8;

/// What happened when the arrived GIF frames were taken.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GifLoad {
    /// More frames are coming.
    Loading,
    /// Every frame is here.
    Done,
    /// It couldn't be read, or had no frames.
    Failed(String),
}

impl GifEditor {
    /// An editor for `path` whose frames are read on a worker thread.
    pub fn start(path: PathBuf, ctx: Option<egui::Context>) -> Self {
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = export::gif_frames(&p, 480);
            let wake = |c: &Option<egui::Context>| {
                if let Some(c) = c {
                    c.request_repaint();
                }
            };
            match r {
                Ok(frames) => {
                    for (img, ms) in frames {
                        if tx.send(GifMsg::Frame(img, ms)).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(GifMsg::Done(Ok(())));
                }
                Err(e) => {
                    let _ = tx.send(GifMsg::Done(Err(e)));
                }
            }
            wake(&ctx);
        });
        Self::loading_from(path, rx)
    }

    fn loading_from(path: PathBuf, rx: Receiver<GifMsg>) -> Self {
        GifEditor { path, frames: Vec::new(), size: vec2(320.0, 240.0), index: 0, playing: true, shown_at: Instant::now(), trim: (0, 0), loading: Some(rx) }
    }

    /// Turn at most `max` arrived frames into textures. The kept range follows the frames
    /// while they arrive, unless it was changed.
    pub fn take_frames(&mut self, ctx: &egui::Context, max: usize) -> GifLoad {
        let Some(rx) = &self.loading else { return GifLoad::Done };
        let mut ended = None;
        for _ in 0..max {
            match rx.try_recv() {
                Ok(GifMsg::Frame(img, ms)) => {
                    // Still "every frame" (not changed by the person): take this one in too.
                    let follow = self.frames.is_empty() || self.trim == (0, self.frames.len() - 1);
                    if self.frames.is_empty() {
                        self.size = vec2(img.width() as f32, img.height() as f32);
                    }
                    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                    let i = self.frames.len();
                    self.frames.push((ctx.load_texture(format!("gif-{i}"), ci, TextureOptions::LINEAR), ms));
                    if follow {
                        self.trim.1 = self.frames.len() - 1;
                    }
                }
                Ok(GifMsg::Done(r)) => {
                    ended = Some(r);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return GifLoad::Loading,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    ended = Some(Err("reading stopped".into()));
                    break;
                }
            }
        }
        match ended {
            None => GifLoad::Loading,
            Some(r) => {
                self.loading = None;
                match r {
                    Err(e) => GifLoad::Failed(e),
                    Ok(()) if self.frames.is_empty() => GifLoad::Failed(String::new()),
                    Ok(()) => GifLoad::Done,
                }
            }
        }
    }
}

pub struct SnagApp {
    /// The ribbon, Studio, languages, updates and the HTTP server for AI apps.
    pub(crate) x: ribbon::Extra,
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
    /// The windows and screens for the menus, made on a worker thread.
    pub(crate) win_cache: work::ListCache<WindowInfo>,
    pub(crate) mon_cache: work::ListCache<MonitorInfo>,
    /// Library adds, saves and copies after a capture (and other slow work), in order.
    pub(crate) jobs: work::Jobs<Apply>,
    /// Files that a background save is still writing.
    pub(crate) saving: std::collections::HashSet<PathBuf>,
    mcp_debounce: work::Debounce<Option<(u16, String)>>,
    textures_swept: Instant,
    overlay: Option<Overlay>,
    restore_pos: Option<Pos2>,
    /// The window is hidden to the tray (closed to the tray, started there, or hidden after a
    /// capture): egui can't tell, so Snagster keeps track itself.
    in_tray: bool,
    /// The capture overlay was opened from the tray, so it goes back there afterwards.
    overlay_from_tray: bool,
    /// The capture started while the window was minimized or in the tray (not hidden by the
    /// capture itself): cancelling puts it back out of the way instead of opening the editor.
    started_away: bool,
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
    video_tx: Sender<crate::video::VideoDone>,
    video_rx: Receiver<crate::video::VideoDone>,
    pub(crate) job_pause: Option<Arc<AtomicBool>>,
    pub(crate) videos: Vec<crate::video::VideoDone>,
    /// Pictures and videos already on this computer, for the Recent strip's "Earlier".
    pub(crate) earlier: earlier::Earlier,

    pub(crate) gif_editor: Option<GifEditor>,
    pub(crate) gif_open: bool,
    pub(crate) mini: Option<Mini>,
    mini_since: Instant,
    /// When the recording bar opened (its size is asked for again in the first moments).
    mini_entered: Instant,
    /// Where the recording bar's buttons were last frame: a press there is a click, never a
    /// drag of the bar.
    mini_buttons: Vec<egui::Rect>,
    /// A window size asked for after the recording bar, and since when: asked again until the
    /// window has it (Wayland may apply an older size after it).
    size_wanted: Option<(Vec2, Instant)>,
    mini_paused: Option<Instant>,
    /// Where (when the system says: not on Wayland) and how big the window was before the
    /// recording bar.
    mini_restore: Option<(Option<Pos2>, Vec2)>,
    /// The window's size the last time it was shown normally (no overlay, no bar, not full
    /// screen): what comes back after the recording bar where the position is unknown.
    normal_size: Option<Vec2>,
    pub(crate) interval: Option<IntervalJob>,
    pub(crate) schedule: Option<(chrono::NaiveTime, CaptureKind)>,
    pub(crate) timer_open: bool,
    // library
    pub(crate) library_open: bool,
    pub(crate) lib_items: Vec<library::Item>,
    pub(crate) lib_loaded: bool,
    /// A library scan under way: the folder it reads and its result.
    pub(crate) lib_scan_rx: Option<(String, Receiver<Vec<library::Item>>)>,
    /// Captures added to the library while a scan was under way (merged into its result).
    pub(crate) lib_added: Vec<library::Item>,
    pub(crate) lib_query: String,
    pub(crate) lib_sort: usize,
    pub(crate) lib_app: String,
    pub(crate) run_at_startup: Option<bool>,
    pub(crate) lib_thumbs: HashMap<PathBuf, TextureHandle>,
    pub(crate) lib_thumb_rx: Option<Receiver<(PathBuf, RgbaImage)>>,
    pub(crate) lib_tag_edit: Option<(usize, String)>,
    // ui helpers
    ui_fonts: fonts::UiFonts,
    /// Stamp pictures by emoji and size (`None`: the font can't draw it).
    stamp_tex: HashMap<(char, u32), Option<TextureHandle>>,
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
    #[allow(dead_code)]
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
    /// How far the scrolling capture has got.
    scroll_progress: Option<Arc<capture::ScrollProgress>>,
}

impl SnagApp {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<PathBuf>, open: Option<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
        let check_run = screenshot.is_some() || UI_TEST.load(Ordering::Relaxed);
        let settings = Settings::load(check_run);
        fonts::start_scan(cc.egui_ctx.clone());
        crate::instance::set_wake(&cc.egui_ctx);
        // Check runs (button test, README pictures) keep every box inside the one window they look at.
        if screenshot.is_some() || UI_TEST.load(Ordering::Relaxed) {
            cc.egui_ctx.set_embed_viewports(true);
        }
        // Start looking for cloud folders now, so the Share tab has them when it opens.
        crate::settings::cloud_folders_now(Some(&cc.egui_ctx));
        let mut app = Self::blank(settings, !check_run, screenshot);
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
            // The tray's menu and the Mac menu bar send their items here, tray icon or not.
            let ctx2 = cc.egui_ctx.clone();
            crate::tray::listen(app.tray_tx.clone(), move || ctx2.request_repaint());
            app.apply_tray(&cc.egui_ctx);
        }
        if let Some(p) = open {
            app.open_path(&p);
        }
        app
    }

    /// The window's state with nothing open and nothing started (no window, threads, hotkeys
    /// or files touched), so `new` and the unit tests share it.
    fn blank(settings: Settings, persist: bool, screenshot: Option<PathBuf>) -> Self {
        let (cap_tx, cap_rx) = channel();
        let (hk_tx, hk_rx) = channel();
        let (ocr_tx, ocr_rx) = channel();
        let (gif_tx, gif_rx) = channel();
        let (video_tx, video_rx) = channel();
        let (tray_tx, tray_rx) = channel();
        Self {
            x: ribbon::Extra::default(),
            saved_settings: settings.clone(),
            settings,
            persist,
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
            win_cache: work::ListCache::new(Duration::from_secs(3)),
            mon_cache: work::ListCache::new(Duration::from_secs(3)),
            jobs: work::Jobs::new(),
            saving: std::collections::HashSet::new(),
            mcp_debounce: work::Debounce::new(),
            textures_swept: Instant::now(),
            overlay: None,
            restore_pos: None,
            in_tray: false,
            overlay_from_tray: false,
            started_away: false,
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
            video_tx,
            video_rx,
            job_pause: None,
            videos: Vec::new(),
            earlier: Default::default(),

            gif_editor: None,
            gif_open: false,
            mini: None,
            mini_since: Instant::now(),
            mini_entered: Instant::now(),
            mini_buttons: Vec::new(),
            size_wanted: None,
            mini_paused: None,
            mini_restore: None,
            normal_size: None,
            interval: None,
            schedule: None,
            timer_open: false,
            library_open: false,
            lib_items: Vec::new(),
            lib_loaded: false,
            lib_scan_rx: None,
            lib_added: Vec::new(),
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
            scroll_progress: None,
        }
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
            Action::MenuCapture if !capture::menus_supported() => {
                self.status = crate::i18n::t(capture::MENUS_UNAVAILABLE).into();
            }
            Action::MenuCapture => {
                let d = self.settings.menu_delay_s.max(1);
                self.status = crate::i18n::tf("Open the menu now: Snagster captures it in {seconds} seconds.", &[("seconds", &d.to_string())]);
                self.start_capture_with(ctx, CaptureKind::Menus, d);
            }
            Action::GrabText => self.start_capture(ctx, pick(CutShape::Rect, PickPurpose::GrabText)),
            Action::ColorPicker => self.start_capture(ctx, CaptureKind::ColorPicker),
            Action::Ruler => self.start_capture(ctx, CaptureKind::Ruler),
            Action::RecordGif => {
                if self.is_recording() {
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
        if capture::is_scrolling(&kind) && !capture::scrolling_supported() {
            self.status = crate::i18n::t(capture::SCROLL_UNAVAILABLE).into();
            return;
        }
        self.commit_text();
        self.ask_screen_access_once();
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
        let outer = ctx.input(|i| i.viewport().outer_rect);
        self.restore_pos = outer.map(|r| pos2(r.min.x * ppp, r.min.y * ppp));
        let at = outer.map(|r| ((r.center().x * ppp) as i32, (r.center().y * ppp) as i32)).unwrap_or((0, 0));
        // Hidden to the tray counts as out of the way too (no minimizing, no settle time).
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false) || self.in_tray;
        self.hidden_for_capture = minimized;
        self.started_away = minimized;
        let mut delay = 60;
        // Wayland: Snagster never minimizes itself for a capture: it couldn't come back (only the
        // person can un-minimize a Wayland window), and the capture seemed frozen until they
        // clicked Snagster in the dock. Plain pictures go to the desktop's own screenshot tool.
        if self.settings.hide_window && !minimized && !self_minimize_blocked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.hidden_for_capture = true;
            delay = self.settings.hide_settle_ms;
        }
        delay += delay_s * 1000;
        capture::spawn(kind.clone(), delay, at, ctx.clone(), self.cap_tx.clone());
        self.status = if delay_s > 0 { format!("Capturing in {delay_s} seconds...") } else { "Capturing...".into() };
        self.capturing = Some(kind);
    }

    /// A desktop notification for work that finished while Snagster was out of the way (in the
    /// tray, minimized or behind other windows): GNOME's and KDE's guidelines, Linux only for now.
    fn notify_away(&self, ctx: &egui::Context, title: &str, path: &std::path::Path) {
        let focused = ctx.input(|i| i.viewport().focused);
        if !notify_when(focused, self.in_tray) {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            crate::portal::notify("finished", title, &name);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (title, path);
    }

    /// Hide the window to the tray (it keeps running there).
    pub(crate) fn hide_to_tray(&mut self, ctx: &egui::Context) {
        self.in_tray = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    /// Make the window visible again (after the tray).
    pub(crate) fn unhide(&mut self, ctx: &egui::Context) {
        self.in_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    }

    pub(crate) fn show_window(&mut self, ctx: &egui::Context) {
        self.unhide(ctx);
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
                    let mut ov = Overlay::new(mode, shape, shot);
                    // Buttons, lists and panels inside the windows are read on a worker.
                    if self.settings.detect_windows && self.settings.detect_objects {
                        ov.read_controls(ctx);
                    }
                    self.overlay = Some(ov);
                    self.overlay_from_tray = self.in_tray;
                    self.in_tray = false;
                    for cmd in overlay_show_commands(at) {
                        ctx.send_viewport_cmd(cmd);
                    }
                }
                Ok(shot) => {
                    let show = !self.quiet_capture && (!self.hidden_for_capture || self.settings.after_show_editor);
                    if show {
                        self.show_window(ctx);
                    } else if self.tray.is_some() {
                        self.hide_to_tray(ctx);
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
                    self.status = self.grab_failed("Capture failed: {error}", &e);
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
        } else if std::mem::take(&mut self.overlay_from_tray) && self.tray.is_some() {
            self.hide_to_tray(ctx);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    /// A new picture arrived: put it in Recent and the library and run the after-capture steps
    /// (or the preset's).
    pub(crate) fn finish_capture(&mut self, mut img: RgbaImage, title: &str, app: &str, kind: &str, cursor: Option<Pos2>) {
        let preset = self.pending_preset.take().and_then(|i| self.settings.presets.get(i).cloned());
        let panel_job = self.pending_panel.take();
        let opts = self.settings.effect_options(effects::info_line(&[app, title]));
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
        let mut notes = vec![crate::i18n::tf("Captured {width} x {height}.", &[("width", &w.to_string()), ("height", &h.to_string())])];
        // Only Recent is filled here; the library, the clipboard and the save folder are
        // written by the job queue in this order (an error shows in the status line).
        if self.settings.after_library {
            let meta = library::Meta { title: title.to_string(), app: app.to_string(), kind: kind.into(), ..Default::default() };
            self.add_to_library(move || img, meta);
        }
        let (copy, save, pin) = match &preset {
            Some(p) => (p.copy, p.save, p.pin),
            None => (self.settings.after_copy, self.settings.after_save, self.settings.after_pin),
        };
        if copy {
            self.copy_quietly(true);
            notes.push(crate::i18n::t("Copied.").into());
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
            notes.push(crate::i18n::tf("Preset \"{name}\".", &[("name", &p.name)]));
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
                self.leave_overlay(&ctx, !self.started_away);
            }
            Outcome::Color(c) => {
                let hex = format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
                self.settings.color = c;
                let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(hex.clone()));
                self.status = crate::i18n::tf("Picked {color}. It's copied and set as the drawing color.", &[("color", &hex)]);
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
                        self.start_scroll(&ctx, desktop, horizontal, false, ov.shot.space);
                    }
                    AreaAction::ManualScroll => {
                        self.leave_overlay(&ctx, true);
                        self.start_scroll(&ctx, desktop, false, true, ov.shot.space);
                    }
                    AreaAction::RecordGif => {
                        self.leave_overlay(&ctx, true);
                        if !self.stream_area_picked(&ctx, desktop) {
                            self.start_record(&ctx, desktop);
                        }
                    }
                }
            }
        }
    }

    // ---------- background jobs: scrolling, GIF, steps, text ----------

    fn enter_mini(&mut self, ctx: &egui::Context, kind: Mini) {
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        self.mini_restore = match (outer, inner) {
            (Some(o), Some(i)) => Some((Some(pos2(o.min.x * ppp, o.min.y * ppp)), i.size())),
            // Wayland: no position, and the window may still be full screen from the overlay.
            _ => self.normal_size.map(|s| (None, s)),
        };
        let (bw, bh) = (MINI_BAR_SIZE.x, MINI_BAR_SIZE.y);
        if let Ok((mx, my, mw, mh)) = capture::monitor_under_cursor() {
            let x = (mx + mw as i32) as f32 - (bw + 24.0) * ppp;
            let y = (my + mh as i32) as f32 - (bh + 72.0) * ppp;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(200.0, 40.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(bw, bh)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(x / ppp, y / ppp)));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        }
        self.mini = Some(kind);
        self.mini_since = Instant::now();
        self.mini_entered = Instant::now();
    }

    fn leave_mini(&mut self, ctx: &egui::Context) {
        self.mini = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(MIN_SIZE));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        if let Some((pos, size)) = self.mini_restore.take() {
            let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            self.size_wanted = Some((size, Instant::now()));
            if let Some(pos) = pos {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(pos.x / ppp, pos.y / ppp)));
            }
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub(crate) fn stop_job(&mut self) {
        if let Some(s) = &self.job_stop {
            s.store(true, Ordering::Relaxed);
        }
    }

    /// The options a scrolling capture gets from the settings.
    pub(crate) fn scroll_options(&self, horizontal: bool, manual: bool, space: capture::detect::Space) -> capture::ScrollOptions {
        capture::ScrollOptions::from_settings(&self.settings, horizontal, manual, true, space)
    }

    fn start_scroll(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32), horizontal: bool, manual: bool, space: capture::detect::Space) {
        // Where Snagster can't turn other programs' mouse wheels, the person scrolls.
        let can = manual || capture::can_scroll_itself();
        let opts = capture::ScrollOptions::from_settings(&self.settings, horizontal, manual, can, space);
        self.scroll_manual = opts.manual;
        let stop = Arc::new(AtomicBool::new(false));
        self.job_stop = Some(stop.clone());
        let progress = Arc::new(capture::ScrollProgress::default());
        self.scroll_progress = Some(progress.clone());
        let (tx, rx) = channel();
        self.scroll_rx = Some(rx);
        let ctx2 = ctx.clone();
        self.enter_mini(ctx, Mini::Scroll);
        std::thread::spawn(move || {
            // Let Snagster's bar settle before the first frame.
            std::thread::sleep(Duration::from_millis(400));
            let r = capture::scroll_capture(rect, &opts, stop, Some(progress));
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        self.status = if !can {
            crate::i18n::t("Snagster can't scroll other programs here, so scroll it yourself and press Stop.").into()
        } else if manual {
            crate::i18n::t("Scroll the page yourself; press Stop at the end.").into()
        } else {
            crate::i18n::t("Scrolling and stitching… keep the mouse still.").into()
        };
    }

    /// Whether a GIF or video recording is running.
    pub(crate) fn is_recording(&self) -> bool {
        matches!(self.mini, Some(Mini::Gif) | Some(Mini::Video))
    }

    /// Record an area in the chosen format (a video file, or a GIF).
    pub(crate) fn start_record(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32)) {
        if self.settings.rec_format == crate::video::VideoFormat::Gif {
            self.start_gif(ctx, rect);
        } else {
            self.start_video(ctx, rect);
        }
    }

    /// The options a video recording of `rect` gets from the settings (the same ones the MCP
    /// server's record_video uses). GIFs don't come here: `start_record` sends them to
    /// `start_gif`.
    pub(crate) fn video_opts(&self, rect: (i32, i32, u32, u32)) -> crate::video::VideoOpts {
        crate::video::VideoOpts::from_settings(&self.settings, rect)
    }

    pub(crate) fn start_video(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32)) {
        self.start_video_to(ctx, rect, None);
    }

    /// Start a video recording; `path` replaces the usual file in the Recordings folder.
    pub(crate) fn start_video_to(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32), path: Option<PathBuf>) {
        if let Err(e) = crate::video::even_size(rect.2, rect.3) {
            self.status = e;
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        self.job_stop = Some(stop.clone());
        self.job_pause = Some(pause.clone());
        let mut opts = self.video_opts(rect);
        if let Some(p) = path {
            opts.path = p;
        }
        self.enter_mini(ctx, Mini::Video);
        crate::video::spawn(opts, stop, pause, Some(ctx.clone()), self.video_tx.clone());
        self.status = crate::i18n::tf("Recording video at {fps} frames per second…", &[("fps", &self.settings.video_fps.to_string())]);
    }

    /// The display under the mouse pointer (or the first one), as a desktop area.
    pub(crate) fn screen_under_pointer(&self) -> Option<(i32, i32, u32, u32)> {
        capture::monitor_under_cursor().ok()
    }

    /// Pause or carry on with a video recording.
    pub(crate) fn toggle_pause(&mut self) {
        if let Some(p) = &self.job_pause {
            let now = !p.load(Ordering::Relaxed);
            p.store(now, Ordering::Relaxed);
            if now {
                self.mini_paused = Some(Instant::now());
            } else if let Some(t) = self.mini_paused.take() {
                self.mini_since += t.elapsed();
            }
        }
    }

    pub(crate) fn paused(&self) -> bool {
        self.job_pause.as_ref().is_some_and(|p| p.load(Ordering::Relaxed))
    }

    /// Open a file with the program the computer uses for it.
    pub(crate) fn open_file(&mut self, path: &std::path::Path) {
        #[cfg(windows)]
        let r = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn();
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg(path).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let r = std::process::Command::new("xdg-open").arg(path).spawn();
        if let Err(e) = r {
            self.status = crate::i18n::tf("Couldn't open it: {error}", &[("error", &e.to_string())]);
        }
    }

    /// Show a file selected in the file manager.
    pub(crate) fn reveal(&mut self, path: &std::path::Path) {
        #[cfg(windows)]
        let r = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg("-R").arg(path).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let r = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
        if let Err(e) = r {
            self.status = crate::i18n::tf("Couldn't open the folder: {error}", &[("error", &e.to_string())]);
        }
    }

    pub(crate) fn start_gif(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32)) {
        let stop = Arc::new(AtomicBool::new(false));
        self.job_stop = Some(stop.clone());
        let path = crate::video::recording_path(&self.settings.video_folder, "Recording", "gif");
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
        self.status = "Recording a GIF…".into();
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
        self.status = crate::i18n::tf("Recorded {count} steps. Use File > Export guide to make a document.", &[("count", &self.guide.len().to_string())]);
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.scroll_rx
            && let Ok(r) = rx.try_recv()
        {
            self.scroll_rx = None;
            self.scroll_progress = None;
            self.job_stop = None;
            self.leave_mini(ctx);
            match r {
                Ok(img) => self.finish_capture(img, "Scrolling capture", "", "Scrolling", None),
                Err(e) => self.status = self.grab_failed("Scrolling capture failed: {error}", &e),
            }
        }
        while let Ok(done) = self.gif_rx.try_recv() {
            self.job_stop = None;
            if self.mini == Some(Mini::Gif) {
                self.leave_mini(ctx);
            }
            match done.result {
                Ok(()) => {
                    self.status = crate::i18n::tf("Recorded {frames} frames ({seconds} s). Open Tools > GIF recordings to trim or grab frames.", &[("frames", &done.frames.to_string()), ("seconds", &format!("{:.1}", done.seconds))]);
                    self.notify_away(ctx, crate::i18n::t("GIF saved"), &done.path);
                    self.recordings.insert(0, Recording { path: done.path.clone(), frames: done.frames, seconds: done.seconds });
                    self.open_gif(ctx, done.path);
                }
                Err(e) => self.status = self.grab_failed("Recording failed: {error}", &e),
            }
        }
        while let Ok(done) = self.video_rx.try_recv() {
            self.job_stop = None;
            self.job_pause = None;
            self.mini_paused = None;
            if self.mini == Some(Mini::Video) {
                self.leave_mini(ctx);
            }
            match &done.result {
                Ok(()) => {
                    let name = done.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let how = if done.hardware { crate::i18n::tf("graphics chip: {encoder}", &[("encoder", &done.encoder)]) } else { done.encoder.clone() };
                    self.status = crate::i18n::tf(
                        "Saved {name}: {seconds} s, {frames} frames, {width}x{height} ({encoder}). Record > Play last video.",
                        &[("name", &name), ("seconds", &format!("{:.1}", done.seconds)), ("frames", &done.frames.to_string()), ("width", &done.width.to_string()), ("height", &done.height.to_string()), ("encoder", &how)],
                    );
                    if !done.note.is_empty() {
                        self.status = format!("{} {}.", self.status, recording_note(&done.note));
                    }
                    self.notify_away(ctx, crate::i18n::t("Recording saved"), &done.path);
                    self.videos.insert(0, done);
                    self.earlier.expire();
                }
                Err(e) => {
                    self.status = self.grab_failed("Video recording failed: {error}", e);
                    if done.frames > 0 {
                        self.videos.insert(0, done);
                    self.earlier.expire();
                    }
                }
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
            let for_library = self.settings.after_library.then(|| d.clone());
            self.entries.push(Entry::new(caption, d));
            self.guide.push(self.entries.len() - 1);
            if let Some(doc) = for_library {
                self.add_to_library(move || render::compose(&doc), library::Meta { title, app: ev.app, kind: "Step".into(), ..Default::default() });
            }
        }
        while let Ok(r) = self.ocr_rx.try_recv() {
            self.ocr_busy = false;
            self.ocr_done(r);
        }
        self.poll_gif_load(ctx);
        for apply in self.jobs.finished(32) {
            apply(self, ctx);
        }
    }

    /// Turn the frames of a GIF being opened into textures, a few per frame.
    fn poll_gif_load(&mut self, ctx: &egui::Context) {
        let Some(ed) = &mut self.gif_editor else { return };
        match ed.take_frames(ctx, GIF_FRAMES_PER_PAINT) {
            GifLoad::Loading => ctx.request_repaint(),
            GifLoad::Done => {}
            GifLoad::Failed(e) => {
                self.gif_editor = None;
                self.status = if e.is_empty() { "That GIF has no frames.".into() } else { format!("Couldn't open the GIF: {e}") };
            }
        }
    }

    /// Add a picture (made by `make`, on the worker) to the library. The open library gets
    /// the new item without reading the whole folder again.
    pub(crate) fn add_to_library(&mut self, make: impl FnOnce() -> RgbaImage + Send + 'static, meta: library::Meta) {
        let folder = self.settings.library_folder.clone();
        self.jobs.queue(move || {
            let img = make();
            let r = library::add_item(std::path::Path::new(&folder), &img, meta);
            let thumb = r.as_ref().ok().and_then(|it| library::thumbnail_of(&it.path, &img, 192));
            Box::new(move |app: &mut SnagApp, ctx: &egui::Context| app.library_added(ctx, &folder, r, thumb))
        });
    }

    fn library_added(&mut self, ctx: &egui::Context, folder: &str, r: Result<library::Item, String>, thumb: Option<RgbaImage>) {
        let item = match r {
            Ok(item) => item,
            Err(e) => {
                self.status = crate::i18n::tf("Not added to the library: {error}", &[("error", &e)]);
                return;
            }
        };
        if folder != self.settings.library_folder {
            return;
        }
        if let Some(t) = thumb {
            let ci = egui::ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw());
            self.lib_thumbs.insert(item.path.clone(), ctx.load_texture("lib-thumb", ci, TextureOptions::LINEAR));
        }
        if self.lib_scan_rx.is_some() {
            self.lib_added.push(item);
        } else if self.lib_loaded {
            library::insert_newest(&mut self.lib_items, item);
        }
        // Otherwise the library hasn't been read yet and will find it then.
    }

    pub(crate) fn run_ocr(&mut self, purpose: OcrPurpose, image: RgbaImage) {
        self.ocr_busy = true;
        self.status = "Reading text…".into();
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
                    self.status = crate::i18n::tf("Read {count} lines of text; they're copied to the clipboard.", &[("count", &lines.len().to_string())]);
                    self.grabbed_text = Some(text);
                }
            }
            OcrPurpose::Redact(i) => {
                let kinds = ocr::RedactKinds::from_settings(&self.settings);
                let block = self.settings.blur_block;
                if let Some(e) = self.entries.get_mut(i) {
                    match ocr::redact(&e.doc, &lines, kinds, block) {
                        None => self.status = "Nothing sensitive was found.".into(),
                        Some((d, found)) => {
                            e.push_undo();
                            e.doc = d;
                            self.status = redact_status(&found);
                        }
                    }
                }
            }
            OcrPurpose::Simplify(i) => {
                let out = ocr::simplify(&r.image, &lines);
                if let Some(e) = self.entries.get_mut(i) {
                    e.push_undo();
                    e.doc = Doc::new(out);
                    self.status = crate::i18n::tf("Simplified: {count} lines of text became bars. Undo brings them back.", &[("count", &lines.len().to_string())]);
                }
            }
        }
    }

    /// Open a GIF in the GIF editor. Its frames are read on a worker thread and shown as
    /// they arrive (reading a long recording takes seconds).
    pub(crate) fn open_gif(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.gif_editor = Some(GifEditor::start(path, Some(ctx.clone())));
        self.gif_open = true;
        ctx.request_repaint();
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

    /// Open a picture or a Snagster project. The file is read and decoded on the job queue (a
    /// big picture, a tall scrolling capture or a file on a cloud drive takes a while, and the
    /// window must keep answering); several opened at once arrive in the order asked.
    pub(crate) fn open_path(&mut self, p: &std::path::Path) {
        let p = p.to_path_buf();
        self.status = crate::i18n::tf("Opening {path}…", &[("path", &p.display().to_string())]);
        self.jobs.queue(move || {
            let r = read_picture_file(&p);
            Box::new(move |app: &mut SnagApp, _: &egui::Context| app.opened(&p, r))
        });
    }

    /// A file read by `open_path` arrived.
    fn opened(&mut self, p: &std::path::Path, r: Result<(String, Doc, bool), String>) {
        match r {
            Ok((name, d, project)) => {
                self.remember_file(p);
                self.add_entry(name, d);
                if let Some(e) = self.entry() {
                    e.path = Some(p.to_path_buf());
                }
                let path = p.display().to_string();
                self.status = if project { crate::i18n::tf("Opened project {path}", &[("path", &path)]) } else { crate::i18n::tf("Opened {path}", &[("path", &path)]) };
            }
            Err(e) => self.status = crate::i18n::tf("Couldn't open {path}: {error}", &[("path", &p.display().to_string()), ("error", &e)]),
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

    /// Read the clipboard's picture on a thread of its own (the program that put it there
    /// is asked for it, and may be slow or hung), then hand it to `then`.
    fn with_clipboard_picture(&mut self, then: impl FnOnce(&mut SnagApp, Option<RgbaImage>) + Send + 'static) {
        self.jobs.spawn(move || {
            let got = clipboard_picture();
            Box::new(move |app: &mut SnagApp, _: &egui::Context| then(app, got))
        });
    }

    pub(crate) fn paste(&mut self) {
        self.with_clipboard_picture(|app, got| match got {
            Some(rgba) => {
                let (w, h) = rgba.dimensions();
                let name = app.next_name("Pasted");
                app.add_entry(name, Doc::new(rgba));
                app.status = crate::i18n::tf("Pasted a {width} x {height} picture.", &[("width", &w.to_string()), ("height", &h.to_string())]);
            }
            None => app.status = "There's no picture on the clipboard.".into(),
        });
    }

    pub(crate) fn copy(&mut self) {
        self.copy_quietly(false);
    }

    /// Copy the current picture with its marks. The picture is put together and handed to
    /// the clipboard on the job queue; `quiet` leaves the status line alone unless it fails.
    pub(crate) fn copy_quietly(&mut self, quiet: bool) {
        self.commit_text();
        let Some(e) = self.entry() else { return };
        let doc = e.doc.clone();
        self.jobs.queue(move || {
            let out = render::compose(&doc);
            let (w, h) = out.dimensions();
            let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: out.into_raw().into() };
            let r = arboard::Clipboard::new().and_then(|mut c| c.set_image(data)).map_err(|e| e.to_string());
            Box::new(move |app: &mut SnagApp, _: &egui::Context| match r {
                Ok(()) if !quiet => app.status = crate::i18n::tf("Copied the {width} x {height} picture. Paste it anywhere.", &[("width", &w.to_string()), ("height", &h.to_string())]),
                Ok(()) => {}
                Err(e) => app.status = crate::i18n::tf("Couldn't copy: {error}", &[("error", &e.to_string())]),
            })
        });
    }

    pub(crate) fn save_to(&mut self, i: usize, path: PathBuf) {
        let result = export::save_doc(&self.entries[i].doc, &self.entries[i].name, &path, &self.settings);
        self.status = match result {
            Ok(()) => {
                self.entries[i].path = Some(path.clone());
                self.remember_file(&path);
                crate::i18n::tf("Saved {path}", &[("path", &path.display().to_string())])
            }
            Err(e) => crate::i18n::tf("Couldn't save: {error}", &[("error", &e.to_string())]),
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
            self.save_in_background(i, path);
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
            self.save_in_background(i, path);
        }
    }

    /// Save straight into the save folder with the file-name pattern. The file is written on
    /// the job queue; the status line says where, or why it couldn't be.
    pub(crate) fn quick_save(&mut self) {
        self.commit_text();
        let Some(i) = self.current else { return };
        let title = self.entries[i].name.clone();
        let path = work::free_path(self.settings.next_save_path(&title), &self.saving);
        self.save_in_background(i, path);
    }

    /// `save_to`, on the job queue.
    pub(crate) fn save_in_background(&mut self, i: usize, path: PathBuf) {
        let Some(e) = self.entries.get(i) else { return };
        let (id, doc, name, settings) = (e.id, e.doc.clone(), e.name.clone(), self.settings.clone());
        self.saving.insert(path.clone());
        self.status = crate::i18n::tf("Saved {path}", &[("path", &path.display().to_string())]);
        self.jobs.queue(move || {
            let r = export::save_doc(&doc, &name, &path, &settings);
            Box::new(move |app: &mut SnagApp, _: &egui::Context| app.saved(id, path, r))
        });
    }

    fn saved(&mut self, id: u64, path: PathBuf, r: Result<(), String>) {
        self.saving.remove(&path);
        self.earlier.expire();
        match r {
            Ok(()) => {
                if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                    e.path = Some(path.clone());
                }
                self.remember_file(&path);
            }
            Err(e) => self.status = crate::i18n::tf("Couldn't save: {error}", &[("error", &e.to_string())]),
        }
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
            self.status = crate::i18n::tf("Couldn't open the folder: {error}", &[("error", &e.to_string())]);
        }
    }

    /// Open the picture in the computer's default program for its file type.
    pub(crate) fn open_externally(&mut self) {
        let path = std::env::temp_dir().join(format!("snag-share-{}.png", chrono::Local::now().format("%H%M%S")));
        self.with_picture_file(path, |app, path| app.open_file_externally(&path));
    }

    fn open_file_externally(&mut self, path: &std::path::Path) {
        #[cfg(windows)]
        let r = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(&path).spawn();
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg(&path).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let r = std::process::Command::new("xdg-open").arg(&path).spawn();
        if let Err(e) = r {
            self.status = crate::i18n::tf("Couldn't open it: {error}", &[("error", &e.to_string())]);
        }
    }

    /// The pictures for a guide: the recorded steps, or else everything in Recent.
    pub(crate) fn guide_items(&self) -> Vec<(String, RgbaImage)> {
        drawn(self.guide_docs())
    }

    /// The guide's pictures with their names, not drawn yet: copies to draw on the job queue.
    pub(crate) fn guide_docs(&self) -> Vec<(String, Doc)> {
        let idx: Vec<usize> = if self.guide.iter().all(|i| *i < self.entries.len()) && !self.guide.is_empty() {
            self.guide.clone()
        } else {
            (0..self.entries.len()).collect()
        };
        idx.into_iter().map(|i| (self.entries[i].name.clone(), self.entries[i].doc.clone())).collect()
    }

    /// Make a new picture on the job queue and add it to Recent as `name` when it is done:
    /// drawing many pictures and laying them out takes seconds, and the window must keep
    /// answering. `make` gives the picture and the status line to show.
    pub(crate) fn make_picture(&mut self, name: &'static str, make: impl FnOnce() -> Option<(RgbaImage, String)> + Send + 'static) {
        self.status = crate::i18n::t("Working on it…").into();
        self.jobs.queue(move || {
            let r = make();
            Box::new(move |app: &mut SnagApp, _: &egui::Context| match r {
                Some((img, status)) => {
                    let n = app.next_name(name);
                    app.add_entry(n, Doc::new(img));
                    app.status = status;
                }
                None => app.status = crate::i18n::t("Couldn't make the picture.").into(),
            })
        });
    }

    /// Write a file on the job queue, then `then` with it; the status line says where, or why not.
    fn write_in_background(&mut self, path: PathBuf, make: impl FnOnce(&std::path::Path) -> Result<(), String> + Send + 'static, then: fn(&mut SnagApp, &std::path::Path)) {
        self.status = crate::i18n::t("Working on it…").into();
        self.jobs.queue(move || {
            let r = make(&path);
            Box::new(move |app: &mut SnagApp, _: &egui::Context| match r {
                Ok(()) => {
                    app.status = crate::i18n::tf("Saved {path}", &[("path", &path.display().to_string())]);
                    then(app, &path);
                }
                Err(e) => app.status = crate::i18n::tf("Couldn't save: {error}", &[("error", &e)]),
            })
        });
    }

    pub(crate) fn export_guide(&mut self, format: &str) {
        self.commit_text();
        let docs = self.guide_docs();
        if docs.is_empty() {
            self.status = "There's nothing to put in a guide yet.".into();
            return;
        }
        let title = self.guide_title.clone();
        if format == "page" {
            self.make_picture("Guide", move || {
                let items = drawn(docs);
                let img = effects::guide_page(&title, &items, 1200)?;
                Some((img, crate::i18n::tf("Made a one-page guide from {count} pictures.", &[("count", &items.len().to_string())])))
            });
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
        let quality = self.settings.jpeg_quality;
        let pdf = ext == "pdf";
        self.write_in_background(
            path,
            move |path| {
                let items = drawn(docs);
                if pdf {
                    let pages: Vec<(Option<String>, RgbaImage)> = items.into_iter().map(|(c, i)| (Some(c), i)).collect();
                    export::pdf(&pages, true, quality).and_then(|b| std::fs::write(path, b).map_err(|e| e.to_string()))
                } else {
                    export::html_guide(&title, &items).and_then(|s| std::fs::write(path, s).map_err(|e| e.to_string()))
                }
            },
            |_, _| {},
        );
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
        // Text being typed belongs to the picture as it is now: settle it first (as undo does),
        // or it would later be written over, or delete, whatever mark has its index in the
        // redone picture.
        self.commit_text();
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

    /// The effect settings with the current picture's name as capture info (what the MCP
    /// server's apply_effect uses for the same settings and name).
    pub(crate) fn effect_options(&self) -> EffectOptions {
        let name = self.current.and_then(|i| self.entries.get(i)).map(|e| e.name.as_str()).unwrap_or_default();
        self.settings.effect_options(effects::info_line(&[name]))
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
        self.status = crate::i18n::tf("{effect} applied ({width} x {height}). Undo brings the marks back.", &[("effect", crate::i18n::t(eff.label())), ("width", &w.to_string()), ("height", &h.to_string())]);
    }

    fn combine(&mut self, vertical: bool) {
        self.commit_text();
        if self.entries.len() < 2 {
            self.status = "Combining needs at least two pictures in Recent.".into();
            return;
        }
        let docs: Vec<Doc> = self.entries.iter().map(|e| e.doc.clone()).collect();
        self.make_picture("Combined", move || {
            let imgs: Vec<RgbaImage> = docs.iter().map(render::compose).collect();
            let out = effects::combine(&imgs, vertical, 16)?;
            Some((out, crate::i18n::tf("Combined {count} pictures.", &[("count", &imgs.len().to_string())])))
        });
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
        self.finish_draw_from(start, cur, points, shift, false);
    }

    /// `finish_draw`; with `centred` (Alt) boxes grow from the press outwards.
    fn finish_draw_from(&mut self, start: Pos2, cur: Pos2, points: Vec<Pos2>, shift: bool, centred: bool) {
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
                let rect = doc::drag_box(start, b, centred);
                Some(if tool == Tool::Rect {
                    Shape::Rect { rect, color, width, fill, radius, shadow }
                } else {
                    Shape::Ellipse { rect, color, width, fill, shadow }
                })
            }
            Tool::Spotlight if far => Some(Shape::Spotlight { rect: Rect::from_two_pos(start, cur) }),
            Tool::Magnify if far => Some(Shape::Magnify { center: start, radius: start.distance(cur), zoom: self.settings.magnify_zoom }),
            Tool::Pen if !points.is_empty() => Some(Shape::Pen { points, color, width, shadow }),
            Tool::Highlight if self.settings.highlight_box => far.then(|| Shape::Marker { rect: doc::drag_box(start, cur, centred), color }),
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
                    self.status = crate::i18n::tf("Cut out a band; the picture is now {width} x {height}.", &[("width", &w.to_string()), ("height", &h.to_string())]);
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
                    self.status = crate::i18n::tf("Cropped to {width} x {height}.", &[("width", &w.to_string()), ("height", &h.to_string())]);
                }
                None
            }
            _ => None,
        };
        if let Some(s) = shape {
            // A mark with handles stays picked, so it can be adjusted straight away.
            let adjustable = !s.grips().is_empty();
            let e = &mut self.entries[ci];
            e.push_undo();
            e.doc.shapes.push(s);
            if adjustable {
                self.selected = Some(e.doc.shapes.len() - 1);
            }
        }
    }

    /// The handle of the picked mark under `p` (within `reach`, in picture pixels), nearest first.
    fn grip_at(&self, p: Pos2, reach: f32) -> Option<(usize, doc::Grip)> {
        let idx = self.selected?;
        let s = self.entries.get(self.current?)?.doc.shapes.get(idx)?;
        s.grips()
            .into_iter()
            .map(|(g, q)| (g, q.distance(p)))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(g, _)| (idx, g))
    }

    /// The topmost mark under `p` that the current tool owns (see `tool_owns`).
    fn pick_own(&self, p: Pos2) -> Option<usize> {
        let i = self.pick(p)?;
        tool_owns(self.tool, &self.entries[self.current?].doc.shapes[i]).then_some(i)
    }

    /// A press that starts a drag at `p`: take the picked mark's handle, pick up a mark the
    /// tool owns, or start drawing.
    fn begin_drag(&mut self, p: Pos2, reach: f32) {
        if grabs(self.tool)
            && let Some((idx, grip)) = self.grip_at(p, reach)
            && let Some(ci) = self.current
        {
            let orig = Box::new(self.entries[ci].doc.shapes[idx].clone());
            self.drag = Drag::Grip { idx, grip, orig, pushed: false };
            return;
        }
        match self.tool {
            Tool::Select => {
                self.selected = self.pick(p);
                self.drag = match self.selected {
                    Some(idx) => Drag::Move { idx, last: p, pushed: false },
                    None => Drag::None,
                };
            }
            Tool::Text | Tool::Step | Tool::Stamp | Tool::Fill => {}
            t => match self.pick_own(p).filter(|_| grabs(t)) {
                Some(idx) => {
                    self.selected = Some(idx);
                    self.drag = Drag::Move { idx, last: p, pushed: false };
                }
                None => {
                    self.selected = None;
                    self.drag = Drag::Draw { start: p, cur: p, points: vec![p] };
                }
            },
        }
    }

    /// Move the taken handle to `p` (the first move is undoable as one step).
    fn drag_grip(&mut self, p: Pos2, keep: bool) {
        let Some(ci) = self.current else { return };
        if let Drag::Grip { idx, grip, orig, pushed } = &mut self.drag {
            let new = orig.with_grip(*grip, p, keep);
            let e = &mut self.entries[ci];
            if !*pushed {
                *pushed = true;
                e.push_undo();
            }
            if let Some(s) = e.doc.shapes.get_mut(*idx) {
                *s = new;
            }
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
        // Every system's standard keys (docs/DESIGN-GUIDELINES.md in the private notes): Save,
        // Save as with Shift, Print; the menus show these same keys.
        if consume(Modifiers::COMMAND | Modifiers::ALT, Key::S) {
            self.quick_save();
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            self.save_as();
        }
        if consume(Modifiers::COMMAND, Key::S) {
            self.save();
        }
        if consume(Modifiers::COMMAND, Key::N) {
            self.dialog = Some(crate::windows::Dialog::new_image());
        }
        if consume(Modifiers::COMMAND, Key::O) {
            self.open_dialog();
        }
        if consume(Modifiers::COMMAND, Key::Comma) {
            self.settings_open = true;
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::P) {
            self.pin_current();
        }
        if consume(Modifiers::COMMAND, Key::P) && self.current.is_some() {
            self.print();
        }
        // Quit: Ctrl+Q on Linux, Cmd+Q on a Mac (Windows uses Alt+F4, which the system handles).
        if crate::platform::system() != crate::platform::System::Windows && consume(Modifiers::COMMAND, Key::Q) {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // GNOME's key for the shortcuts window.
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::Questionmark) || consume(Modifiers::COMMAND, Key::Questionmark) {
            self.shortcuts_open = true;
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
        // A mark just drawn stays picked to adjust it, but copying then still means the picture:
        // marks are copied and cut with the selection tool.
        let mark_picked = self.selected.is_some() && self.tool == Tool::Select;
        if cut_ev && mark_picked {
            self.cut_mark();
        }
        if (copy_ev || consume(Modifiers::COMMAND, Key::C)) && self.current.is_some() {
            if mark_picked {
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
        // (Ctrl/Cmd + a number switches Studio scenes, not the zoom.)
        if ctx.input(|i| i.key_pressed(Key::Num0) && !i.modifiers.command) {
            self.set_zoom(ctx, None);
        }
        if ctx.input(|i| i.key_pressed(Key::Num1) && !i.modifiers.command) {
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

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let has = self.current.is_some();
        if item(ui, "New from clipboard", "Ctrl+V") {
            self.paste();
        }
        if item(ui, "New image…", "Ctrl+N") {
            self.dialog = Some(crate::windows::Dialog::new_image());
        }
        ui.menu_button(crate::i18n::t("New from template"), |ui| self.template_menu(ui));
        if item(ui, "Open picture or project…", "Ctrl+O") {
            self.open_dialog();
        }
        ui.menu_button(crate::i18n::t("Open recent"), |ui| {
            if self.settings.recent_files.is_empty() {
                ui.label(RichText::new(crate::i18n::t("Nothing opened or saved yet")).color(TEXT_DIM));
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
        if item(ui, "Import pictures…", "") {
            if let Some(files) = rfd::FileDialog::new().add_filter("Pictures and Snagster projects", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "ico", "snag"]).pick_files() {
                for f in files {
                    self.open_path(&f);
                }
            }
        }
        if item(ui, "Library…", "Ctrl+L") {
            self.library_open = true;
        }
        ui.separator();
        if item_on(ui, has, "Save", "Ctrl+S") {
            self.save();
        }
        if item_on(ui, has, "Save as…", "Ctrl+Shift+S") {
            self.save_as();
        }
        if item_on(ui, has, "Rename…", "F2") {
            if let Some(i) = self.current {
                self.dialog = Some(crate::windows::Dialog::Rename(self.entries[i].name.clone()));
            }
        }
        if item_on(ui, has, crate::platform::trash_text("Move to Recycle Bin", "Move to Trash"), "") {
            self.recycle_current();
        }
        if item_on(ui, !self.entries.is_empty(), "Export pictures…", "") {
            self.dialog = Some(crate::windows::Dialog::export(self.entries.len(), &self.settings));
        }
        if item_on(ui, has, "Print…", "Ctrl+P") {
            self.print();
        }
        if item_on(ui, has, "Save as Snagster project (editable marks)…", "") {
            self.save_project_as();
        }
        if item_on(ui, has, "Quick save to folder", "Ctrl+Alt+S") {
            self.quick_save();
        }
        ui.menu_button(crate::i18n::t("Export guide"), |ui| {
            ui.label(RichText::new(crate::i18n::t("From recorded steps, or all of Recent")).color(TEXT_DIM));
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Title"));
                ui.text_edit_singleline(&mut self.guide_title);
            });
            if item(ui, "PDF document…", "") {
                self.export_guide("pdf");
            }
            if item(ui, "Web page…", "") {
                self.export_guide("html");
            }
            if item(ui, "One-page picture", "") {
                self.export_guide("page");
            }
        });
        ui.separator();
        let path = self.current.and_then(|i| self.entries[i].path.clone());
        if item_on(ui, path.is_some(), "Copy file path", "") {
            self.copy_file_path();
        }
        if item(ui, "Open save folder", "") {
            let f = PathBuf::from(&self.settings.save_folder);
            self.open_folder(&f);
        }
        ui.separator();
        if item(ui, "Settings…", "Ctrl+,") {
            self.settings_open = true;
        }
        ui.separator();
        let (quit, quit_keys) = crate::platform::quit_item();
        if item(ui, quit, quit_keys) {
            // Ends Snagster even when closing the window would keep it in the tray.
            self.quitting = true;
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
        ui.menu_button(crate::i18n::t("Fun shapes"), |ui| {
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
        if item(ui, "Capture from a file…", "") {
            self.open_dialog();
        }
        ui.menu_button(crate::i18n::t("Fixed size"), |ui| {
            for (w, h) in [(640, 480), (800, 600), (1024, 768), (1280, 720), (1920, 1080), (500, 500)] {
                if item(ui, &format!("{w} x {h}"), "") {
                    pick = Some(CaptureKind::Fixed(w, h));
                }
            }
        });
        ui.separator();
        ui.menu_button(crate::i18n::t("Screens"), |ui| {
            let ctx = ui.ctx().clone();
            self.mon_cache.get(Some(&ctx), capture::monitors);
            if !self.mon_cache.loaded() {
                ui.spinner();
            }
            for (n, m) in self.mon_cache.items().iter().enumerate() {
                let label = format!("{}: {}  {} x {}{}", n + 1, m.name, m.width, m.height, if m.primary { "  (main)" } else { "" });
                if ui.button(label).clicked() {
                    pick = Some(CaptureKind::Monitor(m.id));
                    ui.close();
                }
            }
        });
        ui.menu_button(crate::i18n::t("Window list"), |ui| {
            if let Some(k) = self.window_list(ui) {
                pick = Some(k);
            }
        });
        ui.menu_button(crate::i18n::t("Presets"), |ui| {
            let mut run = None;
            for (i, p) in self.settings.presets.iter().enumerate() {
                let keys = if p.combo.is_set() { p.combo.label() } else { String::new() };
                if item(ui, &p.name, &keys) {
                    run = Some(i);
                }
            }
            ui.separator();
            if item(ui, "Edit presets…", "") {
                self.settings_open = true;
                self.settings_tab = SettingsTab::Presets;
            }
            if item(ui, "Import presets…", "") {
                self.import_presets();
            }
            if item(ui, "Export all presets…", "") {
                self.export_presets(None);
            }
            if let Some(i) = run {
                self.run_preset(ctx, i);
            }
        });
        ui.separator();
        ui.menu_button(crate::i18n::t("Delay"), |ui| {
            for d in [0u64, 1, 2, 3, 5, 10, 15, 30] {
                let label = if d == 0 { "No delay".to_string() } else { format!("{d} seconds") };
                if ui.radio(self.settings.delay_s == d, label).clicked() {
                    self.settings.delay_s = d;
                    ui.close();
                }
            }
        });
        if item(ui, "Timer: scheduled and repeated captures…", "") {
            self.timer_open = true;
        }
        ui.menu_button(crate::i18n::t("After capture"), |ui| {
            ui.checkbox(&mut self.settings.after_show_editor, crate::i18n::t("Show Snagster with the picture"));
            ui.checkbox(&mut self.settings.after_library, crate::i18n::t("Add to the library"));
            ui.checkbox(&mut self.settings.after_copy, crate::i18n::t("Copy to clipboard"));
            ui.checkbox(&mut self.settings.after_save, crate::i18n::t("Save to the save folder"));
            ui.checkbox(&mut self.settings.after_pin, crate::i18n::t("Pin to screen"));
        });
        ui.checkbox(&mut self.settings.include_cursor, crate::i18n::t("Include the mouse pointer (movable)"));
        ui.checkbox(&mut self.settings.hide_window, crate::i18n::t("Hide Snagster while capturing"));
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
        let rec_label = if self.is_recording() {
            "Stop recording".to_string()
        } else {
            format!("Record an area ({}, {} fps)...", self.settings.rec_format.ext().to_uppercase(), if self.settings.rec_format == crate::video::VideoFormat::Gif { self.settings.gif_fps } else { self.settings.video_fps })
        };
        if item(ui, &rec_label, &hk(&self.settings, Action::RecordGif)) {
            self.do_action(ctx, Action::RecordGif);
        }
        if item_on(ui, !self.is_recording(), "Record the whole screen", "") {
            ui.close();
            if let Some(r) = self.screen_under_pointer() {
                self.start_record(ctx, r);
            }
        }
        if self.mini == Some(Mini::Video) && item(ui, if self.paused() { "Resume recording" } else { "Pause recording" }, "") {
            self.toggle_pause();
        }
        ui.menu_button(crate::i18n::t("Record as"), |ui| {
            for f in crate::video::VideoFormat::ALL {
                if f.available() && ui.radio(self.settings.rec_format == f, f.label()).clicked() {
                    self.settings.rec_format = f;
                }
            }
        });
        ui.menu_button(crate::i18n::t("Frames per second"), |ui| {
            for fps in [1, 5, 10, 15, 24, 30, 50, 60, 90, 120] {
                if ui.radio(self.settings.video_fps == fps, format!("{fps} fps")).clicked() {
                    self.settings.video_fps = fps;
                }
            }
            ui.add(egui::Slider::new(&mut self.settings.video_fps, 1..=120).text("fps"));
        });
        let last = self.videos.first().map(|v| v.path.clone());
        if item_on(ui, last.is_some(), "Play last video", "") {
            ui.close();
            if let Some(p) = &last {
                self.open_file(p);
            }
        }
        if item_on(ui, last.is_some(), "Show last video in its folder", "") {
            ui.close();
            if let Some(p) = &last {
                self.reveal(p);
            }
        }
        if item(ui, "Video settings…", "") {
            ui.close();
            self.settings_open = true;
            self.settings_tab = SettingsTab::Recording;
        }
        ui.separator();
        let step_label = if self.mini == Some(Mini::Steps) { "Stop step recording" } else { "Record steps (a picture per click)" };
        if item(ui, step_label, &hk(&self.settings, Action::RecordSteps)) {
            self.do_action(ctx, Action::RecordSteps);
        }
        ui.separator();
        if item(ui, "GIF recordings: trim, frames, combine…", "") {
            self.gif_open = true;
        }
        if item_on(ui, self.entries.len() >= 2, "Make a GIF slideshow from Recent", "") {
            self.slideshow(ctx);
        }
    }

    /// A GIF of the pictures in Recent, made on the job queue (it opens when it's written).
    pub(crate) fn slideshow(&mut self, _ctx: &egui::Context) {
        let docs: Vec<Doc> = self.entries.iter().map(|e| e.doc.clone()).collect();
        let max_w = self.settings.gif_max_width;
        let secs = self.settings.slideshow_s;
        let ms = (secs.clamp(0.2, 30.0) * 1000.0) as u32;
        let path = work::free_path(crate::video::recording_path(&self.settings.video_folder, "Slideshow", "gif"), &self.saving);
        self.saving.insert(path.clone());
        self.jobs.queue(move || {
            let imgs: Vec<RgbaImage> = docs.iter().map(render::compose).collect();
            let r = export::slideshow_gif(&imgs, max_w, secs, &path);
            Box::new(move |app: &mut SnagApp, ctx: &egui::Context| {
                app.saving.remove(&path);
                match r {
                    Ok(n) => {
                        app.recordings.insert(0, Recording { path: path.clone(), frames: n as u32, seconds: n as f32 * ms as f32 / 1000.0 });
                        app.status = crate::i18n::tf("Made a {count}-picture slideshow GIF.", &[("count", &n.to_string())]);
                        app.open_gif(ctx, path);
                    }
                    Err(e) => app.status = crate::i18n::tf("Couldn't make the slideshow: {error}", &[("error", &e.to_string())]),
                }
            })
        });
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        let (can_undo, can_redo, has_marks) = self
            .current
            .map(|i| {
                let e = &self.entries[i];
                (!e.undo.is_empty(), !e.redo.is_empty(), !e.doc.shapes.is_empty())
            })
            .unwrap_or((false, false, false));
        if item(ui, "Editor preferences…", "Ctrl+,") {
            self.settings_open = true;
        }
        ui.separator();
        if item_on(ui, can_undo, "Undo", "Ctrl+Z") {
            self.undo();
        }
        if item_on(ui, can_redo, "Redo", redo_keys()) {
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
        if item_on(ui, has, "Grab text…", "") {
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
        ui.menu_button(crate::i18n::t("Tool"), |ui| {
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
        if item_on(ui, has && !busy, "Smart redact (blur emails, phone numbers…)", "")
            && let Some(i) = self.current
        {
            let img = render::compose(&self.entries[i].doc);
            self.run_ocr(OcrPurpose::Redact(i), img);
        }
        if item_on(ui, has, "Smart redact settings…", "") {
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
        if item_on(ui, has, "Resize image…", "") {
            if let Some(i) = self.current {
                let (w, h) = self.entries[i].doc.image.dimensions();
                self.dialog = Some(crate::windows::Dialog::Resize { w, h, keep: true, ratio: w as f32 / h.max(1) as f32 });
            }
        }
        if item_on(ui, has, "Trim flat edges", "") {
            self.image_op("Trimmed", effects::trim);
        }
        if item_on(ui, has, "Canvas size and color…", "") {
            if let Some(i) = self.current {
                let (w, h) = self.entries[i].doc.image.dimensions();
                self.dialog = Some(crate::windows::Dialog::Canvas { w: w + 80, h: h + 80, color: [255, 255, 255, 255] });
            }
        }
        ui.separator();
        if item(ui, "Effect settings…", "") {
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
        if item(ui, "Grab text from the screen…", &self.settings.binding(Action::GrabText).label_if_set()) {
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
        if item_on(ui, has, "Pin picture to screen", "Ctrl+Shift+P") {
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
            self.copy_color();
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
        if item_on(ui, has, "Email…", "") {
            self.email();
        }
        if item_on(ui, !self.entries.is_empty(), "Microsoft Word document…", "") {
            self.export_word();
        }
        if item_on(ui, has, "Print…", "") {
            self.print();
        }
        if item_on(ui, has, "Open in the default picture program", "") {
            self.open_externally();
        }
        if item_on(ui, has, "Save to the save folder", "Ctrl+Alt+S") {
            self.quick_save();
        }
        let path = self.current.and_then(|i| self.entries[i].path.clone());
        if item_on(ui, path.is_some(), "Show the file in its folder", "")
            && let Some(p) = path
        {
            self.open_folder(&p);
        }
        ui.separator();
        ui.label(RichText::new(crate::i18n::t("Cloud folders (point the library or save folder here to sync):")).color(TEXT_DIM));
        let clouds = crate::settings::cloud_folders_now(Some(ui.ctx()));
        if clouds.is_empty() {
            ui.label(RichText::new(crate::i18n::t("No OneDrive, Dropbox, Google Drive or iCloud folder found.")).color(TEXT_DIM));
        }
        for (name, p) in clouds {
            if item_on(ui, has, &format!("Save to {name}"), "")
                && let Some(i) = self.current
            {
                let title = self.entries[i].name.clone();
                let path = self.settings.cloud_save_path(&p, &title);
                self.save_in_background(i, path);
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
        ui.menu_button(crate::i18n::t("Customize toolbar"), |ui| {
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
            if ui.button(crate::i18n::t("Show all")).clicked() {
                self.settings.hidden_tools.clear();
            }
        });
        ui.checkbox(&mut self.settings.show_side_panel, crate::i18n::t("Show options panel"));
        ui.checkbox(&mut self.settings.show_tray, crate::i18n::t("Show Recent pictures"));
        if item(ui, "Capture panel", "") {
            self.enter_panel(ctx);
        }
    }

    // ---------- panels ----------

    fn tool_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        egui::Grid::new("tools").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            let shown: Vec<Tool> = Tool::ALL.into_iter().filter(|t| !self.settings.hidden_tools.iter().any(|h| h == t.label())).collect();
            for (n, t) in shown.into_iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(54.0, 44.0), Sense::click());
                let selected = self.tool == t;
                let bg = if selected {
                    accent()
                } else if resp.hovered() {
                    PANEL_HI
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect.shrink(1.0), CornerRadius::same(8), bg);
                let fg = if selected { Color32::WHITE } else { Color32::from_rgb(210, 214, 225) };
                let icon = Rect::from_center_size(rect.center() - vec2(0.0, 7.0), vec2(20.0, 20.0));
                tool_icon(ui.painter(), icon, t, fg, self.color());
                // Long names in some languages get a smaller font so they stay inside the button.
                let name = crate::i18n::t(t.label());
                let w = ui.painter().layout_no_wrap(name.to_string(), FontId::proportional(10.0), fg).size().x;
                let size = if w > 52.0 { (10.0 * 52.0 / w).max(7.0) } else { 10.0 };
                ui.painter().text(pos2(rect.center().x, rect.bottom() - 8.0), Align2::CENTER_CENTER, name, FontId::proportional(size), fg);
                let resp = resp.on_hover_text(format!("{} ({})\n{}", crate::i18n::t(t.label()), t.key(), crate::i18n::t(t.hint())));
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
            section(ui, crate::i18n::t("Color"));
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
                        if ui.button(crate::i18n::t("Put the current color here")).clicked() {
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
                ui.label(RichText::new(crate::i18n::t("Theme")).color(TEXT_DIM));
                let mut apply = None;
                egui::ComboBox::from_id_salt("theme").width(120.0).selected_text("Choose…").show_ui(ui, |ui| {
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
                    self.status = crate::i18n::tf("Theme \"{name}\" applied.", &[("name", &t.name)]);
                }
            });
            ui.add_space(10.0);
            section(ui, crate::i18n::t("Size"));
            ui.add(egui::Slider::new(&mut self.settings.width, 1.0..=24.0).step_by(1.0).suffix(" px"));

            let t = self.tool;
            ui.add_space(6.0);
            if matches!(t, Tool::Arrow | Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Pen | Tool::Text | Tool::Callout | Tool::Step) {
                ui.checkbox(&mut self.settings.shadow, crate::i18n::t("Drop shadow"));
            }
            if matches!(t, Tool::Rect | Tool::Ellipse) {
                ui.checkbox(&mut self.settings.fill, crate::i18n::t("Fill boxes and ovals"));
            }
            if t == Tool::Rect {
                ui.label(RichText::new(crate::i18n::t("Rounded corners")).color(TEXT_DIM));
                ui.add(egui::Slider::new(&mut self.settings.corner_radius, 0.0..=60.0).step_by(1.0).suffix(" px"));
            }
            if matches!(t, Tool::Text | Tool::Callout | Tool::Select) {
                self.font_picker(ui);
                if t == Tool::Text {
                    ui.checkbox(&mut self.settings.text_background, crate::i18n::t("Text on a colored box"));
                }
                ui.label(RichText::new(format!("Text size {:.0} px", doc::text_size(self.settings.width))).color(TEXT_DIM));
            }
            if t == Tool::Blur {
                ui.label(RichText::new(crate::i18n::t("Pixel block size")).color(TEXT_DIM));
                ui.add(egui::Slider::new(&mut self.settings.blur_block, 4..=48).suffix(" px"));
            }
            if t == Tool::Magnify {
                ui.label(RichText::new(crate::i18n::t("Magnification")).color(TEXT_DIM));
                ui.add(egui::Slider::new(&mut self.settings.magnify_zoom, 1.5..=6.0).suffix(" x"));
            }
            if t == Tool::Step {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::t("Style")).color(TEXT_DIM));
                    egui::ComboBox::from_id_salt("stepstyle").selected_text(self.settings.step_style.label()).show_ui(ui, |ui| {
                        for s in crate::settings::StepStyle::ALL {
                            ui.selectable_value(&mut self.settings.step_style, s, s.label());
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::t("First")).color(TEXT_DIM));
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
                section(ui, crate::i18n::t("Selected mark"));
                ui.label(RichText::new(crate::i18n::t("Pick a color to recolor it. [ and ] change its size.")).color(TEXT_DIM));
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::t("Smaller")).clicked() {
                        self.scale_selected(1.0 / 1.15);
                    }
                    if ui.button(crate::i18n::t("Bigger")).clicked() {
                        self.scale_selected(1.15);
                    }
                    if ui.button(crate::i18n::t("Delete")).clicked() {
                        self.delete_selected();
                    }
                });
            }

            ui.add_space(10.0);
            section(ui, crate::i18n::t("View"));
            let ctx = ui.ctx().clone();
            ui.horizontal(|ui| {
                if ui.selectable_label(self.zoom.is_none(), crate::i18n::t("Fit")).clicked() {
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
            ui.label(RichText::new(crate::i18n::t(self.tool.label())).strong().color(Color32::WHITE));
            ui.add(egui::Label::new(RichText::new(crate::i18n::t(self.tool.hint())).color(TEXT_DIM).size(12.5)).wrap());
        });
    }

    fn font_picker(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(crate::i18n::t("Font")).color(TEXT_DIM));
        let shown = if self.settings.font.is_empty() { "Default".to_string() } else { self.settings.font.clone() };
        let families = fonts::families();
        let mut chosen: Option<String> = None;
        egui::ComboBox::from_id_salt("font").width(180.0).selected_text(shown).show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.font_query).hint_text("Search fonts"));
            if ui.selectable_label(self.settings.font.is_empty(), crate::i18n::t("Default")).clicked() {
                chosen = Some(String::new());
            }
            match &families {
                None => {
                    ui.label(RichText::new(crate::i18n::t("Reading the installed fonts…")).color(TEXT_DIM));
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
        if ui.selectable_label(self.stamp == POINTER_KEY.to_string(), crate::i18n::t("Mouse pointer")).clicked() {
            self.stamp = POINTER_KEY.to_string();
        }
        let list = ef.groups.get(gi).map(|g| g.1.clone()).unwrap_or_default();
        // Drawing an emoji takes a moment; a big group would freeze the window if all were
        // drawn in one frame. A few are drawn per frame (only those in view), the rest wait
        // as empty squares.
        let mut budget = work::Budget::new(Duration::from_millis(12), 4);
        egui::ScrollArea::vertical().max_height(300.0).id_salt("stamps").show(ui, |ui| {
            egui::Grid::new("stampgrid").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
                for (n, ch) in list.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                    let tex = if !ui.is_rect_visible(rect) {
                        None
                    } else if self.stamp_ready(*ch, 32) || budget.allow() {
                        self.stamp_texture(&ctx, *ch, 32, [40, 40, 40, 255])
                    } else {
                        ui.painter().rect_filled(rect.shrink(6.0), CornerRadius::same(4), PANEL_HI);
                        None
                    };
                    if self.stamp.starts_with(*ch) {
                        ui.painter().rect_filled(rect, CornerRadius::same(4), accent());
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
        if budget.left_some() {
            ctx.request_repaint();
        }
    }

    fn stamp_size(px: u32) -> u32 {
        px.next_power_of_two().clamp(16, 512)
    }

    /// Whether the stamp picture was made already (drawing it again costs nothing).
    fn stamp_ready(&self, ch: char, px: u32) -> bool {
        self.stamp_tex.contains_key(&(ch, Self::stamp_size(px)))
    }

    fn stamp_texture(&mut self, ctx: &egui::Context, ch: char, px: u32, tint: [u8; 4]) -> Option<TextureId> {
        let px = Self::stamp_size(px);
        if let Some(t) = self.stamp_tex.get(&(ch, px)) {
            return t.as_ref().map(|t| t.id());
        }
        let img = if ch == POINTER_KEY { Some(Arc::new(pointer_image(px))) } else { crate::emoji::render(ch, px, tint) };
        // Remember emojis the font can't draw too, so they aren't tried again every frame.
        let h = img.map(|img| {
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            ctx.load_texture(format!("stamp-{}-{px}", ch as u32), ci, TextureOptions::LINEAR)
        });
        let id = h.as_ref().map(|h| h.id());
        self.stamp_tex.insert((ch, px), h);
        id
    }

    fn tray(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width(250.0);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::t("Recent")).strong().color(Color32::WHITE));
                    let snap = self.x.stream.snap.clone();
                    if snap.on_air() && stream_ui::live_badge(ui, &snap).clicked() {
                        let ctx = ui.ctx().clone();
                        self.switch_tab(&ctx, ribbon::RibbonTab::Stream);
                    }
                });
                ui.add(egui::Label::new(RichText::new(crate::i18n::tr(&self.status)).color(TEXT_DIM).size(12.5)).wrap());
                self.screen_access_link(ui);
            });
            ui.separator();
            let mut switch = None;
            let mut remove = None;
            let mut pin = None;
            let mut open_earlier = None;
            let ctx = ui.ctx().clone();
            // Earlier pictures and videos on this computer (read on a worker, every minute).
            if self.persist {
                let s = &self.settings;
                let mut folders = vec![PathBuf::from(&s.library_folder), PathBuf::from(&s.save_folder), PathBuf::from(&s.video_folder)];
                folders.dedup();
                self.earlier.poll(&ctx, folders);
            }
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.entries.is_empty() && self.earlier.items.is_empty() {
                        ui.label(RichText::new(crate::i18n::t("Your captures show up here.")).color(TEXT_DIM));
                    }
                    for i in (0..self.entries.len()).rev() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(124.0, 84.0), Sense::click());
                        // Pictures scrolled out of view aren't drawn (their textures can go).
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
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
                            p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(2.0, accent()), StrokeKind::Inside);
                        } else if resp.hovered() {
                            p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, TEXT_DIM), StrokeKind::Inside);
                        }
                        let name: String = self.entries[i].name.chars().take(18).collect();
                        p.text(pos2(rect.center().x, rect.bottom() - 10.0), Align2::CENTER_CENTER, name, FontId::proportional(11.5), Color32::from_rgb(210, 214, 225));
                        if resp.clicked() {
                            switch = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button(crate::i18n::t("Pin to screen")).clicked() {
                                pin = Some(i);
                                ui.close();
                            }
                            if ui.button(crate::i18n::t("Remove from Recent")).clicked() {
                                remove = Some(i);
                                ui.close();
                            }
                        });
                    }
                    open_earlier = self.earlier_tiles(ui);
                });
            });
            if let Some(item) = open_earlier {
                match self.entries.iter().position(|e| e.path.as_deref() == Some(item.path.as_path())) {
                    Some(i) => switch = Some(i),
                    None if item.video => self.open_file(&item.path),
                    None => self.open_path(&item.path),
                }
            }
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

    /// The Recent strip's "Earlier": pictures and videos already on this computer that aren't
    /// in this session's list. The one clicked, if any.
    fn earlier_tiles(&mut self, ui: &mut egui::Ui) -> Option<earlier::EarlierItem> {
        let open: Vec<PathBuf> = self.entries.iter().filter_map(|e| e.path.clone()).collect();
        let items: Vec<earlier::EarlierItem> = self.earlier.items.iter().filter(|i| !open.contains(&i.path)).cloned().collect();
        if items.is_empty() {
            return None;
        }
        ui.separator();
        ui.label(RichText::new(crate::i18n::t("Earlier")).color(TEXT_DIM).size(12.0));
        let mut pick = None;
        for it in items {
            let (rect, resp) = ui.allocate_exact_size(vec2(124.0, 84.0), Sense::click());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(8), BG_DARK);
            let area = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(116.0, 58.0));
            match self.earlier.thumbs.get(&it.path) {
                Some(tex) => {
                    let size = tex.size_vec2();
                    let s = (area.width() / size.x).min(area.height() / size.y);
                    p.image(tex.id(), Rect::from_center_size(area.center(), size * s), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    let icon = if it.video { crate::look::ph::FILM_STRIP } else { crate::look::ph::IMAGE };
                    p.text(area.center(), Align2::CENTER_CENTER, icon, FontId::new(26.0, crate::look::icons()), TEXT_DIM);
                }
            }
            if it.video {
                let ext = it.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
                p.text(area.right_top() + vec2(-4.0, 4.0), Align2::RIGHT_TOP, ext, FontId::proportional(10.5), Color32::from_rgb(240, 200, 120));
            }
            if resp.hovered() {
                p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, TEXT_DIM), StrokeKind::Inside);
            }
            let name: String = it.name().chars().take(18).collect();
            p.text(pos2(rect.center().x, rect.bottom() - 10.0), Align2::CENTER_CENTER, name, FontId::proportional(11.5), Color32::from_rgb(170, 175, 190));
            let tip = if it.video { crate::i18n::t("Play this video") } else { crate::i18n::t("Open this picture") };
            let resp = resp.on_hover_text(format!("{tip}\n{}", it.path.display()));
            if resp.clicked() {
                pick = Some(it.clone());
            }
            resp.context_menu(|ui| {
                if ui.button(crate::i18n::t("Show in folder")).clicked() {
                    self.open_folder(&it.path);
                    ui.close();
                }
            });
        }
        pick
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
                ("Record video", "MP4, up to 120 fps"),
                ("Open a picture", "Or a .snag project"),
            ][k];
            let x = area.center().x - total / 2.0 + k as f32 * (card_w + 20.0);
            let rect = Rect::from_min_size(pos2(x, top + 140.0), vec2(card_w, 120.0));
            let resp = ui.interact(rect, ui.scope_id().with(("card", k)), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(14), if resp.hovered() { PANEL_HI } else { PANEL });
            if k == 0 {
                p.rect_stroke(rect, CornerRadius::same(14), Stroke::new(2.0, accent()), StrokeKind::Inside);
            }
            let icon = Rect::from_center_size(rect.center() - vec2(0.0, 22.0), vec2(30.0, 30.0));
            match k {
                0 => frame_icon(p, icon, accent(), true),
                1 => {
                    frame_icon(p, icon, accent(), false);
                    p.arrow(icon.center() - vec2(0.0, 6.0), vec2(0.0, 12.0), Stroke::new(2.0, accent()));
                }
                2 => {
                    p.circle_filled(icon.center(), 9.0, Color32::from_rgb(229, 72, 77));
                    p.circle_stroke(icon.center(), 14.0, Stroke::new(2.0, accent()));
                }
                _ => folder_icon(p, icon, accent()),
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
        let col = if resp.hovered() { Color32::WHITE } else { accent() };
        ui.painter().text(link.center(), Align2::CENTER_CENTER, crate::i18n::t("Try the tools on a sample picture"), FontId::proportional(14.0), col);
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
        let fit = fit_scale(area.size(), size, ppp);
        let mut scale = self.zoom.unwrap_or(fit);
        let mut origin = canvas_origin(area.center(), size, scale, self.pan);

        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        let shift = ui.input(|i| i.modifiers.shift);

        if resp.hovered() {
            let zd = ui.input(|i| i.zoom_delta());
            if zd != 1.0 {
                let new_scale = (scale * zd).clamp(0.02, 16.0);
                let cursor = ui.input(|i| i.pointer.hover_pos()).unwrap_or(area.center());
                let (new_origin, pan) = zoom_about(area.center(), size, origin, scale, new_scale, cursor);
                self.pan = pan;
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
        let to_img = |p: Pos2| screen_to_image(p, origin, scale);
        let to_scr = move |p: Pos2| image_to_screen(p, origin, scale);
        let img_rect = Rect::from_min_size(origin, size * scale);

        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let alt = ui.input(|i| i.modifiers.alt);
        // A drag is noticed a few pixels after the press: it starts where the button went down.
        let pressed = ui.input(|i| i.pointer.press_origin()).filter(|q| area.contains(*q));
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(pp) = pressed.or(pointer)
        {
            self.commit_text();
            self.begin_drag(to_img(pp), GRIP_REACH / scale);
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
                Drag::Grip { .. } => self.drag_grip(p, shift),
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
                self.finish_draw_from(start, cur, points, shift, alt);
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
                t if grabs(t) && t != Tool::Callout => self.selected = self.pick_own(p),
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
            if ui.button(crate::i18n::t("Copy picture")).clicked() {
                self.copy();
                ui.close();
            }
            if ui.button(crate::i18n::t("Pin to screen")).clicked() {
                self.pin_current();
                ui.close();
            }
            if self.selected.is_some() && ui.button(crate::i18n::t("Delete mark")).clicked() {
                self.delete_selected();
                ui.close();
            }
            if ui.button(crate::i18n::t("Clear all marks")).clicked() {
                self.clear_marks();
                ui.close();
            }
        });

        if resp.hovered() {
            let at = resp.hover_pos().map(to_img);
            let grip = match &self.drag {
                Drag::Grip { grip, .. } => Some(*grip),
                _ => at.filter(|_| grabs(self.tool)).and_then(|p| self.grip_at(p, GRIP_REACH / scale)).map(|(_, g)| g),
            };
            let over_own = matches!(self.drag, Drag::None) && grabs(self.tool) && at.and_then(|p| self.pick_own(p)).is_some();
            let icon = match grip {
                Some(g) => grip_cursor(g),
                None if over_own || matches!(self.drag, Drag::Move { .. }) => egui::CursorIcon::Move,
                None => match self.tool {
                    Tool::Select => egui::CursorIcon::Default,
                    Tool::Text => egui::CursorIcon::Text,
                    _ => egui::CursorIcon::Crosshair,
                },
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
            let grips = s.grips();
            if let Some(b) = s.boxed() {
                dashed_rect(&painter, Rect::from_two_pos(to_scr(b.min), to_scr(b.max)), accent());
            } else if grips.is_empty() {
                let b = s.bounds();
                dashed_rect(&painter, Rect::from_two_pos(to_scr(b.min), to_scr(b.max)).expand(4.0), accent());
            }
            for (_, q) in grips {
                let r = Rect::from_center_size(to_scr(q), Vec2::splat(9.0));
                painter.rect_filled(r, CornerRadius::same(2), Color32::WHITE);
                painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.5, accent()), StrokeKind::Middle);
            }
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
            let boxed = doc::drag_box(start, cur, alt && matches!(self.tool, Tool::Rect | Tool::Ellipse | Tool::Highlight));
            let preview = match self.tool {
                Tool::Arrow => Some(Shape::Arrow { a: start, b: cur, color, width, shadow, heads: self.settings.arrow_heads }),
                Tool::Line => Some(Shape::Line { a: start, b: cur, color, width, shadow }),
                Tool::Rect => Some(Shape::Rect { rect: boxed, color, width, fill, radius, shadow }),
                Tool::Ellipse => Some(Shape::Ellipse { rect: boxed, color, width, fill, shadow }),
                Tool::Highlight if self.settings.highlight_box => Some(Shape::Marker { rect: boxed, color }),
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
        let enter = ui.input_mut(|i| take_finishing_enter(&mut i.events));
        let escape = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let at = to_scr(t.pos);
        if let TextKind::Callout { tail } = t.kind {
            ui.painter().line_segment([at, to_scr(tail)], Stroke::new(2.0, t.color));
        }
        let font = FontId::new((t.size * scale).max(6.0), family);
        let color = if t.bg { render::halo_color(t.color) } else { t.color };
        let fill = if t.bg { t.color } else { Color32::from_black_alpha(40) };
        let resp = egui::Area::new(egui::Id::unique("text-edit"))
            .fixed_pos(at - vec2(4.0, 4.0))
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, accent()))
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
        // A video counts from its first picture (on Wayland the desktop's "Share screen" dialog
        // comes first).
        let waiting = match self.mini {
            Some(Mini::Video) => !crate::video::recording_live(),
            Some(Mini::Gif) => !capture::gif_live(),
            _ => false,
        };
        if waiting {
            self.mini_since = Instant::now();
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let secs = self.mini_since.elapsed().as_secs();
        // Wayland: leaving the full-screen overlay restores the window's old size after the bar
        // asked for its own, which left a big dark window over the area being recorded. Ask
        // again until the bar has its size.
        // (The content area: on Wayland the window's position, and so its rectangle, is unknown.)
        let have = ctx.input(|i| i.content_rect().size());
        if resize_due(have, MINI_BAR_SIZE, self.mini_entered.elapsed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(MINI_BAR_SIZE));
        }
        // The bar can be dragged by its free space (a Wayland app can't place its own window).
        // (Click and drag: the drag starts once the pointer moves, so a press on Stop isn't
        // handed to the desktop as a window move, which swallowed the click.)
        let bg = ui.interact(ui.max_rect(), ui.make_persistent_id("mini-bar-drag"), Sense::click_and_drag());
        if bg.drag_started() && bar_drag_allowed(ctx.input(|i| i.pointer.press_origin()), &self.mini_buttons) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let mut buttons = Vec::new();
        ui.horizontal_centered(|ui| {
            ui.add_space(10.0);
            let blink = (self.mini_since.elapsed().as_millis() / 500) % 2 == 0;
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 6.0, if blink { Color32::from_rgb(229, 72, 77) } else { Color32::from_rgb(120, 40, 44) });
            let text = match self.mini {
                Some(Mini::Gif) if waiting => "Waiting for screen sharing...".to_string(),
                Some(Mini::Gif) => format!("Recording GIF  {}:{:02}", secs / 60, secs % 60),
                Some(Mini::Video) if self.paused() => "Paused".to_string(),
                Some(Mini::Video) if waiting => "Waiting for screen sharing...".to_string(),
                Some(Mini::Video) => format!("Recording {} at {} fps  {}:{:02}", self.settings.rec_format.ext().to_uppercase(), self.settings.video_fps, secs / 60, secs % 60),
                Some(Mini::Steps) => format!("Recording steps: {} so far", self.guide.len()),
                Some(Mini::Scroll) if self.scroll_manual => "Scroll it yourself, then press Stop".to_string(),
                Some(Mini::Scroll) => match &self.scroll_progress {
                    Some(p) => crate::i18n::tf(
                        "Scrolling: {frames} pictures, {height} pixels. Esc stops.",
                        &[("frames", &p.frames.load(Ordering::Relaxed).to_string()), ("height", &p.length.load(Ordering::Relaxed).to_string())],
                    ),
                    None => "Scrolling and stitching...".to_string(),
                },
                None => String::new(),
            };
            ui.label(RichText::new(text).color(Color32::WHITE).strong());
            // Esc stops a scrolling capture when this bar has the keyboard (on Windows it
            // works anywhere: the capture itself watches the key).
            if self.mini == Some(Mini::Scroll) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.stop_job();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let stop = egui::Button::new(RichText::new(crate::i18n::t("Stop")).color(Color32::WHITE).strong()).fill(Color32::from_rgb(229, 72, 77));
                let stop = ui.add(stop);
                buttons.push(stop.rect);
                if stop.clicked() {
                    match self.mini {
                        Some(Mini::Steps) => self.stop_steps(&ctx),
                        _ => self.stop_job(),
                    }
                }
                if self.mini == Some(Mini::Video) {
                    let label = if self.paused() { "Resume" } else { "Pause" };
                    let pause = ui.button(RichText::new(label).strong());
                    buttons.push(pause.rect);
                    if pause.clicked() {
                        self.toggle_pause();
                    }
                }
            });
        });
        self.mini_buttons = buttons;
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.screenshot else { return };
        job.frames += 1;
        if job.frames == 2 {
            let view = SCREENSHOT_VIEW.lock().map(|v| v.clone()).unwrap_or_default();
            self.show_view(ctx, &view);
        }
        let Some(job) = &mut self.screenshot else { return };
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

impl SnagApp {
    fn logic_body(&mut self, ctx: &egui::Context) {
        self.jobs.wake(ctx);
        self.apply_language(ctx);
        self.poll_updates(ctx);
        // The HTTP server for AI apps runs while it's switched on in Settings.
        // A new port or password is used once it has stopped changing (typing into the field
        // doesn't restart the server on every key); switching it on or off is at once.
        let want = self.settings.mcp_http.then(|| (self.settings.mcp_port, self.settings.mcp_token.clone()));
        let switched = want.is_some() != self.x.mcp_wanted.is_some();
        let editing = ctx.egui_wants_keyboard_input() || ctx.input(|i| i.pointer.any_down());
        let apply = self.mcp_debounce.check(&self.x.mcp_wanted, &want, Instant::now(), Duration::from_secs(1), switched || !editing);
        if self.mcp_debounce.waiting() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if let Some(want) = apply
            && !UI_TEST.load(Ordering::Relaxed)
        {
            self.x.mcp_server = None;
            self.x.mcp_error = None;
            if let Some((port, token)) = &want {
                match crate::mcp_http::start(*port, token) {
                    Ok(s) => self.x.mcp_server = Some(s),
                    Err(e) => self.x.mcp_error = Some(e),
                }
            }
            self.x.mcp_wanted = want;
        }
        if !self.startup_done {
            self.startup_done = true;
            if self.persist {
                match self.settings.start_mode {
                    crate::settings::StartMode::Editor => {}
                    crate::settings::StartMode::Panel => self.enter_panel(ctx),
                    crate::settings::StartMode::Tray => {
                        if self.tray.is_some() {
                            self.hide_to_tray(ctx);
                        }
                    }
                }
            }
        }
        if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
            let ppp = ctx.pixels_per_point();
            capture::remember_window_screen(((m.x * ppp).round() as u32, (m.y * ppp).round() as u32));
        }
        let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if !full && self.mini.is_none() && self.overlay.is_none() && self.size_wanted.is_none() {
            self.normal_size = Some(ctx.input(|i| i.content_rect().size()));
        }
        if let Some((want, since)) = self.size_wanted {
            let have = ctx.input(|i| i.content_rect().size());
            if resize_due(have, want, since.elapsed()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
                ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                self.size_wanted = None;
            }
        }
        self.poll_hotkeys(ctx);
        self.poll_tray(ctx);
        #[cfg(target_os = "macos")]
        self.apply_menu_bar();
        // Close keeps Snagster in the tray only where there is a tray to come back from (stock
        // GNOME has none), and only when the person chose it.
        let keep_running = !self.quitting && self.tray.is_some() && self.settings.close_to_tray && crate::platform::has_tray();
        if ctx.input(|i| i.viewport().close_requested()) && keep_running {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.panel_open {
                self.leave_panel(ctx);
            }
            if capture::desktop_screenshots_only() {
                // Wayland doesn't let a program hide its window: it is minimized instead.
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            } else {
                self.hide_to_tray(ctx);
            }
            self.status = crate::i18n::t(crate::platform::by_system(
                "Snagster keeps running in the corner of the taskbar. Right-click its icon to quit.",
                "Snagster keeps running in the menu bar.",
                "Snagster keeps running in the tray. Right-click its icon to quit.",
            )).to_string();
        }
        // Closing for good ends a stream cleanly (it is unpublished, not cut off).
        if ctx.input(|i| i.viewport().close_requested()) && !keep_running && crate::stream::live::end() {
            crate::stream::live::wait_idle(Duration::from_secs(5));
        }
        self.poll_stream(ctx);
        // Snagster opened again: this one comes forward (and opens the file asked for).
        for ask in crate::instance::take() {
            // A capture asked for from a desktop shortcut: done as if its hotkey was pressed.
            if let crate::instance::Ask::Action(name) = &ask {
                if let Some(a) = crate::settings::Action::ALL.iter().find(|a| format!("{a:?}") == *name) {
                    self.do_action(ctx, *a);
                }
                continue;
            }
            self.unhide(ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            match ask {
                crate::instance::Ask::Open(p) => self.open_path(&p),
                crate::instance::Ask::Command(id, value) => self.run_command(ctx, &id, value.as_deref()),
                // (An action was handled above.)
                crate::instance::Ask::Show | crate::instance::Ask::Action(_) => {}
            }
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

    fn ui_body(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.overlay.is_some() {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.handle_overlay(ui));
            return;
        }
        if self.panel_open {
            egui::CentralPanel::default().frame(Self::panel_frame()).show(ui, |ui| self.panel_ui(ui));
            self.settings_window(&ctx);
            self.stream_confirm_window(&ctx);
        self.screen_access_window(&ctx);
            return;
        }
        if self.mini.is_some() {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(BG_DARK).stroke(Stroke::new(1.0, accent())))
                .show(ui, |ui| self.mini_bar(ui));
            return;
        }
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            self.open_path(&p);
        }
        self.shortcuts(&ctx);

        egui::Panel::top("ribbon")
            .frame(egui::Frame::new().fill(crate::look::BAND))
            .show(ui, |ui| self.ribbon(ui));
        self.language_window(&ctx);
        self.update_window(&ctx);
        self.stream_confirm_window(&ctx);
        self.screen_access_window(&ctx);
        if self.x.tab == ribbon::RibbonTab::Studio {
            self.studio_view(ui);
            self.dialog_window(&ctx);
            self.settings_window(&ctx);
            self.help_windows(&ctx);
            self.library_window(&ctx);
            return;
        }
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

/// Redo's key as each system shows it: Ctrl+Y on Windows, Shift+Ctrl+Z elsewhere (both work).
pub(crate) fn redo_keys() -> &'static str {
    if crate::platform::system() == crate::platform::System::Windows { "Ctrl+Y" } else { "Ctrl+Shift+Z" }
}

pub(crate) fn item(ui: &mut egui::Ui, label: &str, keys: &str) -> bool {
    item_on(ui, true, label, keys)
}

pub(crate) fn item_on(ui: &mut egui::Ui, enabled: bool, label: &str, keys: &str) -> bool {
    let b = egui::Button::new(crate::i18n::tr(label)).shortcut_text(crate::platform::keys(keys));
    let clicked = ui.add_enabled(enabled, b).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn setup_style(ctx: &egui::Context) {
    // Snagster is dark on every system for now. Saying so keeps egui from swapping in its
    // stock style when the system switches between light and dark, and gives every window a
    // dark system title bar to match (instead of a white bar above a dark window).
    ctx.set_theme(egui::Theme::Dark);
    ctx.options_mut(|o| o.sync_window_theme = true);
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG_DARK;
    v.selection.bg_fill = accent();
    v.hyperlink_color = accent();
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

/// The scale that fits a picture of `size` in a canvas of `area` (24 px margin all round),
/// never larger than one picture pixel per screen pixel, never below 2%.
fn fit_scale(area: Vec2, size: Vec2, ppp: f32) -> f32 {
    ((area.x - 48.0) / size.x).min((area.y - 48.0) / size.y).min(1.0 / ppp).max(0.02)
}

/// Where the picture's top-left corner goes: centred in the canvas, then panned.
fn canvas_origin(center: Pos2, size: Vec2, scale: f32, pan: Vec2) -> Pos2 {
    center - size * scale / 2.0 + pan
}

/// Zoom from `scale` to `new_scale` keeping the picture point under `cursor` where it is:
/// the new origin and the pan that gives it.
fn zoom_about(center: Pos2, size: Vec2, origin: Pos2, scale: f32, new_scale: f32, cursor: Pos2) -> (Pos2, Vec2) {
    let new_origin = cursor - (cursor - origin) * (new_scale / scale);
    (new_origin, new_origin - (center - size * new_scale / 2.0))
}

fn screen_to_image(p: Pos2, origin: Pos2, scale: f32) -> Pos2 {
    pos2((p.x - origin.x) / scale, (p.y - origin.y) / scale)
}

fn image_to_screen(p: Pos2, origin: Pos2, scale: f32) -> Pos2 {
    pos2(origin.x + p.x * scale, origin.y + p.y * scale)
}

/// A picture's name made safe as a file name on every system (renaming a saved picture).
fn clean_file_name(name: &str) -> String {
    let bad = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    name.chars().map(|c| if bad.contains(&c) { '_' } else { c }).collect()
}

/// The status line after Smart Redact hid `found` (one name per item, as `ocr::redact` gives).
fn redact_status(found: &[&str]) -> String {
    let mut kinds = found.to_vec();
    kinds.dedup();
    crate::i18n::tf("Blurred {count} items ({kinds}). Undo brings them back.", &[("count", &found.len().to_string()), ("kinds", &kinds.join(", "))])
}

/// A recording's note for the status line, in the person's language. The note itself stays
/// English (AI apps get it as it is); the reason after "Recorded without sound:" comes from the
/// system and is translated only when it is one of Snagster's own texts.
fn recording_note(note: &str) -> String {
    match note.strip_prefix("Recorded without sound: ") {
        Some(reason) => crate::i18n::tf("Recorded without sound: {reason}", &[("reason", &crate::i18n::tr(reason))]),
        None => crate::i18n::tr(note),
    }
}

/// The picture on the clipboard, if there is one.
fn clipboard_picture() -> Option<RgbaImage> {
    let img = arboard::Clipboard::new().and_then(|mut c| c.get_image()).ok()?;
    RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
}

/// The start of a temporary file's name: letters and digits only.
/// A picture file or a Snagster project, read and decoded: its name, the picture, and whether
/// it was a project. Slow for big files; `open_path` runs it on the job queue.
fn read_picture_file(p: &std::path::Path) -> Result<(String, Doc, bool), String> {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "snag" {
        let (name, d) = export::load_project(p).map_err(|e| e.to_string())?;
        return Ok((if name.is_empty() { "Project".into() } else { name }, d, true));
    }
    let img = image::open(p).map_err(|e| e.to_string())?;
    let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Picture".into());
    Ok((name, Doc::new(img.to_rgba8()), false))
}

/// Pictures drawn with their marks (for work on the job queue).
fn drawn(items: Vec<(String, Doc)>) -> Vec<(String, RgbaImage)> {
    items.into_iter().map(|(n, d)| (n, render::compose(&d))).collect()
}

/// The layouts of File > New from template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Template {
    SideBySide,
    Grid2,
    Grid3,
    BeforeAfter,
    Gallery,
}

impl Template {
    /// The new picture's name in Recent.
    fn name(self) -> &'static str {
        match self {
            Template::SideBySide => "Side by side",
            Template::Grid2 | Template::Grid3 => "Grid",
            Template::BeforeAfter => "Before and after",
            Template::Gallery => "Gallery",
        }
    }

    /// The layout of `imgs` (None: too few pictures).
    fn make(self, imgs: &[RgbaImage]) -> Option<RgbaImage> {
        let n = imgs.len();
        match self {
            Template::SideBySide => effects::grid(imgs, n as u32, 24, &[]),
            Template::Grid2 => effects::grid(imgs, 2, 24, &[]),
            Template::Grid3 => effects::grid(imgs, 3, 24, &[]),
            Template::BeforeAfter if n >= 2 => effects::grid(&imgs[n - 2..], 2, 24, &["Before", "After"]),
            Template::BeforeAfter => None,
            Template::Gallery => {
                let labels: Vec<String> = (1..=n).map(|i| format!("{i}.")).collect();
                let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                effects::grid(imgs, 3, 24, &refs)
            }
        }
    }
}

fn temp_base(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
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
        Shape::Marker { rect, color } => {
            p.rect_filled(Rect::from_two_pos(to(rect.min), to(rect.max)), CornerRadius::ZERO, doc::highlight_color(*color));
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
                // The centre, sampling the picture (a textured mesh takes only textured points).
                mesh.vertices.push(egui::epaint::Vertex { pos: c, uv: uvc, color: Color32::WHITE });
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
    p.rect_filled(r, CornerRadius::same((size * 0.25) as u8), ACCENT_DEFAULT);
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
            Err(e) => self.status = crate::i18n::tf("The taskbar-corner icon couldn't be added: {error}", &[("error", &e.to_string())]),
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
                #[cfg(any(target_os = "macos", test))]
                TrayCmd::Menu(run) => self.run_menu(ctx, run),
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
        self.unhide(ctx);
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
            self.hide_to_tray(ctx);
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
                if let (Some((_, folder)), Some(i)) = (crate::settings::cloud_folders_now(None).into_iter().next(), self.current) {
                    let title = self.entries[i].name.clone();
                    let base = self.settings.file_name(&title);
                    let path = crate::settings::unique_path(&folder.join("Snagster"), &base, self.settings.format.ext());
                    self.save_in_background(i, path);
                } else {
                    self.status = "No cloud folder (OneDrive, Dropbox, Google Drive…) was found.".into();
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
        self.with_clipboard_picture(|app, got| match got {
            Some(rgba) => app.add_piece(rgba),
            None => app.status = "There's no picture on the clipboard.".into(),
        });
    }

    fn add_piece(&mut self, rgba: RgbaImage) {
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
            Some(p) if p.parent().is_some_and(|d| d.is_dir()) => self.save_in_background(i, p),
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
            let target = p.with_file_name(format!("{}.{ext}", clean_file_name(name)));
            match std::fs::rename(&p, &target) {
                Ok(()) => {
                    self.entries[i].path = Some(target.clone());
                    self.remember_file(&target);
                }
                Err(e) => {
                    self.status = crate::i18n::tf("Couldn't rename the file: {error}", &[("error", &e.to_string())]);
                    return;
                }
            }
        }
        self.entries[i].name = name.to_string();
        self.status = crate::i18n::tf("Renamed to {name}.", &[("name", &name)]);
    }

    pub(crate) fn recycle_current(&mut self) {
        let Some(i) = self.current else { return };
        let Some(p) = self.entries[i].path.clone() else {
            self.status = "Removed from Recent (it was never saved).".into();
            self.remove_current();
            return;
        };
        // The shell's Recycle Bin can take seconds (a file on a cloud or network drive): on the
        // job queue, and the picture leaves Recent only once its file has gone.
        let id = self.entries[i].id;
        self.jobs.queue(move || {
            let r = trash::delete(&p).map_err(|e| e.to_string());
            Box::new(move |app: &mut SnagApp, _: &egui::Context| app.recycled(id, &p, r))
        });
    }

    fn recycled(&mut self, id: u64, p: &std::path::Path, r: Result<(), String>) {
        if let Err(e) = r {
            self.status = crate::i18n::tf(crate::platform::trash_text("Couldn't move it to the Recycle Bin: {error}", "Couldn't move it to the Trash: {error}"), &[("error", &e)]);
            return;
        }
        let s = p.display().to_string();
        self.settings.recent_files.retain(|f| *f != s);
        self.lib_loaded = false;
        self.status = crate::i18n::tf(crate::platform::trash_text("Moved {path} to the Recycle Bin.", "Moved {path} to the Trash."), &[("path", &s)]);
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            let keep = self.current.filter(|&c| c != i).map(|c| self.entries[c].id);
            self.current = Some(i);
            self.remove_current();
            // Still on the picture that was open, when it wasn't the one removed.
            if let Some(k) = keep {
                self.current = self.entries.iter().position(|e| e.id == k);
            }
        }
    }

    /// Write the current picture to `path` on the job queue (a big picture takes a while to
    /// encode, and the window must not stop answering meanwhile), then `after` with the file on
    /// the window's thread; the status line says why when it can't be written.
    fn with_picture_file(&mut self, path: PathBuf, after: impl FnOnce(&mut SnagApp, PathBuf) + Send + 'static) {
        self.commit_text();
        let Some(e) = self.current.and_then(|i| self.entries.get(i)) else { return };
        let (doc, name, settings) = (e.doc.clone(), e.name.clone(), self.settings.clone());
        self.jobs.queue(move || {
            let r = export::save_doc(&doc, &name, &path, &settings);
            Box::new(move |app: &mut SnagApp, _: &egui::Context| match r {
                Ok(()) => after(app, path),
                Err(e) => app.status = crate::i18n::tf("Couldn't prepare the file: {error}", &[("error", &e.to_string())]),
            })
        });
    }

    /// The current picture as a PNG in the temp folder, then `after` with it (see
    /// `with_picture_file`).
    fn with_temp_png(&mut self, after: impl FnOnce(&mut SnagApp, PathBuf) + Send + 'static) {
        let Some(e) = self.current.and_then(|i| self.entries.get(i)) else { return };
        let base = temp_base(&e.name);
        let path = std::env::temp_dir().join(format!("{base}_{}.png", chrono::Local::now().format("%H%M%S")));
        self.with_picture_file(path, after);
    }

    pub(crate) fn print(&mut self) {
        self.with_temp_png(|app, path| app.print_file(&path));
    }

    fn print_file(&mut self, path: &std::path::Path) {
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
        self.with_temp_png(|app, path| app.email_file(path));
    }

    fn email_file(&mut self, path: PathBuf) {
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
        let docs = self.guide_docs();
        if docs.is_empty() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Word document", &["docx"])
            .set_file_name(format!("{}.docx", self.guide_title))
            .save_file()
        else {
            return;
        };
        let title = self.guide_title.clone();
        self.write_in_background(path, move |path| export::docx(&title, &drawn(docs), path).map_err(|e| e.to_string()), |_, _path| {
            #[cfg(windows)]
            let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(_path).spawn();
        });
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
        self.status = crate::i18n::tf("{what}: now {width} x {height}.", &[("what", &crate::i18n::tr(what)), ("width", &w.to_string()), ("height", &h.to_string())]);
    }

    fn template_menu(&mut self, ui: &mut egui::Ui) {
        let n = self.entries.len();
        ui.label(RichText::new(format!("Made from the {n} pictures in Recent")).color(TEXT_DIM));
        // Nothing is drawn until one is picked (this used to draw every picture on every frame
        // while the menu was open), and then on the job queue.
        if item_on(ui, n >= 1, "Step-by-step guide (one page)", "") {
            self.export_guide("page");
        }
        let mut pick = None;
        for (t, need, label) in [
            (Template::SideBySide, 2, "Side by side"),
            (Template::Grid2, 2, "Grid, 2 across"),
            (Template::Grid3, 3, "Grid, 3 across"),
            (Template::BeforeAfter, 2, "Before and after (last two)"),
            (Template::Gallery, 2, "Numbered gallery"),
        ] {
            if item_on(ui, n >= need, label, "") {
                pick = Some(t);
            }
        }
        if let Some(t) = pick {
            self.make_template(t);
        }
    }

    /// A picture made from everything in Recent, on the job queue.
    pub(crate) fn make_template(&mut self, t: Template) {
        let docs: Vec<Doc> = self.entries.iter().map(|e| e.doc.clone()).collect();
        self.make_picture(t.name(), move || {
            let imgs: Vec<RgbaImage> = docs.iter().map(render::compose).collect();
            Some((t.make(&imgs)?, "Made from a template. Mark it up like any picture.".to_string()))
        });
    }

    /// Per-tool options in the side panel for the new choices, and Quick Styles.
    pub(crate) fn extra_tool_options(&mut self, ui: &mut egui::Ui) {
        use crate::settings::{BlurMode, CalloutShape};
        let t = self.tool;
        if t == Tool::Arrow {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(crate::i18n::t("Heads")).color(TEXT_DIM));
                ui.selectable_value(&mut self.settings.arrow_heads, 0, crate::i18n::t("none"));
                ui.selectable_value(&mut self.settings.arrow_heads, 1, crate::i18n::t("end"));
                ui.selectable_value(&mut self.settings.arrow_heads, 2, crate::i18n::t("both"));
            });
        }
        if matches!(t, Tool::Text | Tool::Callout) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(crate::i18n::t("Align")).color(TEXT_DIM));
                ui.selectable_value(&mut self.settings.text_align, 0, crate::i18n::t("left"));
                ui.selectable_value(&mut self.settings.text_align, 1, crate::i18n::t("center"));
                ui.selectable_value(&mut self.settings.text_align, 2, crate::i18n::t("right"));
            });
        }
        if t == Tool::Text {
            ui.checkbox(&mut self.settings.text_outline, crate::i18n::t("Outline around letters"));
        }
        if t == Tool::Callout {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(crate::i18n::t("Shape")).color(TEXT_DIM));
                for (s, l) in [(CalloutShape::Rounded, "rounded"), (CalloutShape::Rectangle, "box"), (CalloutShape::Oval, "oval")] {
                    ui.selectable_value(&mut self.settings.callout_shape, s, l);
                }
            });
            ui.checkbox(&mut self.settings.callout_outline, crate::i18n::t("Darker outline"));
        }
        if t == Tool::Blur {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(crate::i18n::t("Type")).color(TEXT_DIM));
                for m in BlurMode::ALL {
                    ui.selectable_value(&mut self.settings.blur_mode, m, m.label());
                }
            });
            if ui.button(crate::i18n::t("Smart redact this picture")).clicked()
                && let Some(i) = self.current
            {
                let img = render::compose(&self.entries[i].doc);
                self.run_ocr(OcrPurpose::Redact(i), img);
            }
        }
        if t == Tool::Highlight {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(crate::i18n::t("Shape")).color(TEXT_DIM));
                ui.selectable_value(&mut self.settings.highlight_box, true, crate::i18n::t("box"));
                ui.selectable_value(&mut self.settings.highlight_box, false, crate::i18n::t("freehand"));
            });
        }
        if t == Tool::Eraser {
            ui.label(RichText::new(format!("Eraser width {:.0} px", self.settings.width * 3.0)).color(TEXT_DIM));
        }
        // Quick Styles: saved looks, one click away.
        ui.add_space(8.0);
        section(ui, crate::i18n::t("Quick styles"));
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
                    if ui.button(crate::i18n::t("Remove this style")).clicked() {
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

impl eframe::App for SnagApp {
    // A mistake in one frame must not close the window and lose the person's pictures: it is
    // written to crash.log (by the panic hook) and shown in the status line, and the next frame
    // carries on.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let started = Instant::now();
        crate::drawing::frame(ctx, self.persist);
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.logic_body(ctx))) {
            self.recovered(e);
        }
        self.sweep_textures(started);
        self.watchdog("logic", started.elapsed());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = Instant::now();
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.ui_body(ui))) {
            self.recovered(e);
        }
        self.watchdog("ui", started.elapsed());
    }
}

impl SnagApp {
    /// What is open, for the slow-frame line in crash.log.
    fn open_parts(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        let parts = [
            (self.overlay.is_some(), "capture overlay"),
            (self.panel_open, "capture panel"),
            (self.mini.is_some(), "recording bar"),
            (self.x.studio.is_some(), "Studio"),
            (self.library_open, "Library"),
            (self.lib_scan_rx.is_some(), "library scan"),
            (self.gif_open, "GIF recordings"),
            (self.gif_editor.as_ref().is_some_and(|g| g.loading.is_some()), "GIF loading"),
            (self.settings_open, "Settings"),
            (self.timer_open, "Timer"),
            (self.dialog.is_some(), "a dialog"),
            (self.tool == Tool::Stamp, "stamp picker"),
            (!self.pins.is_empty(), "pins"),
            (self.ocr_busy, "text recognition"),
            (self.jobs.busy() > 0, "background jobs"),
        ];
        for (on, name) in parts {
            if on {
                v.push(name);
            }
        }
        v
    }

    /// A frame that took more than a second (Windows soon calls the window "Not responding")
    /// gets one line in crash.log saying what was open, so the cause can be found.
    fn watchdog(&self, part: &str, took: Duration) {
        if took < work::SLOW_FRAME {
            return;
        }
        let parts = self.open_parts();
        let Some(line) = work::slow_frame_line(part, took, &parts) else { return };
        if self.persist {
            crate::append_crash_log(&line);
        } else {
            eprint!("{line}");
        }
    }

    /// Every few seconds, let go of the textures of pictures in Recent that weren't drawn lately.
    fn sweep_textures(&mut self, now: Instant) {
        if now.saturating_duration_since(self.textures_swept) < Duration::from_secs(5) {
            return;
        }
        self.textures_swept = now;
        let current = self.current;
        for (i, e) in self.entries.iter_mut().enumerate() {
            if Some(i) != current {
                e.drop_stale_texture(now, TEXTURE_KEEP);
            }
        }
    }
    fn recovered(&mut self, e: Box<dyn std::any::Any + Send>) {
        let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        self.status = crate::i18n::tf("Something went wrong and was skipped ({message}). Details are in crash.log (Help > Problem reports).", &[("message", &msg)]);
        // Whatever was half-done is dropped, so it can't fail again every frame.
        self.overlay = None;
        self.text_edit = None;
        self.drag = Drag::None;
    }
}

/// A window's line in the window list: its title (at most 60 characters) and, when known
/// and not already in the title, its program.
pub(crate) fn window_line(w: &capture::WindowInfo) -> String {
    let mut title: String = w.label.chars().take(60).collect();
    if w.label.chars().count() > 60 {
        title.push('…');
    }
    if w.app.is_empty() || w.label.to_lowercase().contains(&w.app.to_lowercase()) { title } else { format!("{title}  ·  {}", w.app) }
}

impl SnagApp {
    /// The open windows to capture, newest list kept by `win_cache`: the one picked, if any.
    pub(crate) fn window_list(&mut self, ui: &mut egui::Ui) -> Option<CaptureKind> {
        let ctx = ui.ctx().clone();
        self.win_cache.get(Some(&ctx), capture::list_windows);
        if !self.win_cache.loaded() {
            ui.spinner();
        } else if self.win_cache.items().is_empty() {
            ui.label(RichText::new(crate::i18n::t("No windows found")).color(TEXT_DIM));
        }
        let mut pick = None;
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for w in self.win_cache.items() {
                if ui.button(window_line(w)).on_hover_text(format!("{} x {}", w.width, w.height)).clicked() {
                    pick = Some(CaptureKind::Window(w.id, w.label.clone()));
                    ui.close();
                }
            }
        });
        pick
    }

    /// The README pictures' scene sound: a microphone in every scene, music in the first.
    fn picture_sounds(&mut self) {
        use crate::audio::SoundInput;
        use crate::studio::SceneSound;
        let st = &mut self.settings.studio;
        st.shared_sounds = vec![SceneSound::new(SoundInput::Computer, "Computer sound"), SceneSound { volume: 1.2, ..SceneSound::new(SoundInput::Microphone { id: String::new() }, "Microphone") }];
        let music = std::env::temp_dir().join("intro-music.wav");
        let _ = std::fs::write(&music, crate::wav::tone(2, 330.0));
        if let Some(s) = st.scenes.first_mut() {
            s.sounds = vec![SceneSound { volume: 0.6, ..SceneSound::new(SoundInput::File { path: music.to_string_lossy().to_string() }, "intro-music.wav") }];
        }
    }

    /// The README pictures' scenes for streaming: an intro, the game and a break.
    fn picture_scenes(&mut self) {
        let st = &mut self.settings.studio;
        let base = st.scenes.first().cloned().unwrap_or_else(|| crate::studio::StudioSettings::default().scenes[0].clone());
        st.scenes = ["Intro", "Game", "Chat", "Break"].iter().map(|n| crate::studio::Scene { name: n.to_string(), ..base.clone() }).collect();
        st.current = 1;
        self.picture_sounds();
    }

    /// Set up a view for `--screenshot --view ...` (README pictures).
    fn show_view(&mut self, ctx: &egui::Context, view: &str) {
        if let Some(code) = view.strip_prefix("lang:") {
            self.settings.language = code.to_string();
            self.x.tab = ribbon::RibbonTab::Home;
            return;
        }
        match view {
            "studio" => {
                let (w, h) = (self.settings.studio.width as f32, self.settings.studio.height as f32);
                if let Some(scene) = self.settings.studio.scenes.first_mut() {
                    scene.items.insert(0, crate::studio::Item { name: "Background".into(), kind: crate::studio::SourceKind::Color { color: [28, 34, 52, 255] }, x: 0.0, y: 0.0, w, h, visible: true });
                    if let Some(screen) = scene.items.iter_mut().find(|i| matches!(i.kind, crate::studio::SourceKind::Display { .. })) {
                        (screen.x, screen.y, screen.w, screen.h) = (w * 0.04, h * 0.06, w * 0.72, h * 0.72);
                        // The README picture shows the sample, not the real desktop.
                        let p = std::env::temp_dir().join("snagster-sample.png");
                        if export::save_image(&render::compose(&sample::sample_doc()), &p, 90).is_ok() {
                            screen.name = "Screen 1".into();
                            screen.kind = crate::studio::SourceKind::Image { path: p.to_string_lossy().to_string() };
                        }
                    }
                    scene.items.push(crate::studio::Item {
                        name: "Title".into(),
                        kind: crate::studio::SourceKind::Text { text: "Live demo".into(), size: 72.0, color: [255, 255, 255, 255] },
                        x: w * 0.04,
                        y: h * 0.82,
                        w: w * 0.6,
                        h: 110.0,
                        visible: true,
                    });
                }
                self.settings.studio.scenes.push(crate::studio::Scene { sounds: Vec::new(), name: "Webcam corner".into(), items: Vec::new() });
                self.picture_sounds();
                self.switch_tab(ctx, ribbon::RibbonTab::Studio);
                if let Some(s) = &mut self.x.studio {
                    s.select(1);
                }
            }
            "record" => self.x.tab = ribbon::RibbonTab::Record,
            "stream" => {
                // Streaming Studio's scenes: scene buttons and the scene's sounds in the band.
                self.picture_scenes();
                self.settings.stream.source = crate::stream::Source::Studio;
                self.x.tab = ribbon::RibbonTab::Stream;
            }
            "panel-stream" => {
                self.settings.panel.tab = 3;
                self.enter_panel(ctx);
            }
            "settings-stream" => {
                self.x.tab = ribbon::RibbonTab::Stream;
                self.settings_open = true;
                self.settings_tab = SettingsTab::Streaming;
                // Multistreaming in the picture: YouTube alongside Twitch (no real keys).
                let id = self.settings.stream.new_also_id();
                self.settings.stream.also = vec![crate::settings::ExtraService { id, service: crate::stream::Service::YouTube, url: String::new(), on: true }];
            }
            "settings-ai" => {
                self.settings_open = true;
                self.settings_tab = SettingsTab::Ai;
            }
            "language" => {
                self.x.lang_open = true;
            }
            _ => {}
        }
    }
}

/// Whether a press at `press` may move the recording bar: not when it was on one of its buttons.
fn bar_drag_allowed(press: Option<egui::Pos2>, buttons: &[egui::Rect]) -> bool {
    press.is_some_and(|p| !buttons.iter().any(|r| r.expand(2.0).contains(p)))
}

/// The recording bar's size.
const MINI_BAR_SIZE: Vec2 = vec2(420.0, 56.0);

/// Whether a window size asked for `since` ago should be asked for again: in the first
/// seconds, while the window is visibly bigger or smaller than `want`.
fn resize_due(have: Vec2, want: Vec2, since: Duration) -> bool {
    since < Duration::from_secs(3) && ((have.x - want.x).abs() > 4.0 || (have.y - want.y).abs() > 4.0)
}

/// Whether finished work gets a desktop notification: only when the window isn't in front (in
/// the tray, or the desktop says another window has the focus). Not known counts as in front.
fn notify_when(focused: Option<bool>, in_tray: bool) -> bool {
    in_tray || focused == Some(false)
}

/// Whether Snagster may not minimize itself before a capture (Linux on Wayland).
fn self_minimize_blocked() -> bool {
    capture::desktop_screenshots_only()
}

/// What brings up the full-screen capture overlay, from a window that is shown, minimized or
/// hidden to the tray. Visible(true) comes first: without it a window hidden to the tray went
/// full screen but stayed invisible, holding the keyboard (Print Screen seemed to freeze).
pub(crate) fn overlay_show_commands(at: Pos2) -> Vec<egui::ViewportCommand> {
    use egui::ViewportCommand as C;
    vec![C::Visible(true), C::Minimized(false), C::OuterPosition(at), C::Fullscreen(true), C::WindowLevel(egui::WindowLevel::AlwaysOnTop), C::Focus]
}

#[cfg(test)]
mod tests {
    #[test]
    fn finished_work_is_notified_only_when_snagster_is_away() {
        assert!(super::notify_when(Some(false), false), "another window has the focus");
        assert!(super::notify_when(None, true), "in the tray");
        assert!(!super::notify_when(Some(true), false), "Snagster is in front: the status line says it");
        assert!(!super::notify_when(None, false), "not known: no notification");
    }

    /// The first click on the recording bar's Stop was lost on GNOME: the press started a
    /// window move, and the desktop kept the release. A press on a button never moves the bar.
    #[test]
    fn a_press_on_the_bars_buttons_never_moves_it() {
        let stop = egui::Rect::from_min_max(pos2(330.0, 14.0), pos2(370.0, 42.0));
        let pause = egui::Rect::from_min_max(pos2(270.0, 14.0), pos2(320.0, 42.0));
        let buttons = [stop, pause];
        assert!(!bar_drag_allowed(Some(pos2(350.0, 28.0)), &buttons));
        assert!(!bar_drag_allowed(Some(pos2(290.0, 20.0)), &buttons));
        // Right at a button's edge still counts as the button.
        assert!(!bar_drag_allowed(Some(pos2(371.0, 28.0)), &buttons));
        // The free space moves it; no press, no move.
        assert!(bar_drag_allowed(Some(pos2(120.0, 28.0)), &buttons));
        assert!(!bar_drag_allowed(None, &buttons));
    }

    /// The recording bar re-asks for its size while the window is still the old size (Wayland
    /// restored it after leaving full screen), and stops asking after a few seconds.
    #[test]
    fn the_recording_bar_asks_for_its_size_until_it_has_it() {
        use super::{MINI_BAR_SIZE, resize_due};
        use std::time::Duration;
        let old = egui::vec2(1165.0, 752.0);
        assert!(resize_due(old, MINI_BAR_SIZE, Duration::from_millis(300)));
        assert!(!resize_due(MINI_BAR_SIZE, MINI_BAR_SIZE, Duration::from_millis(300)), "already the bar's size");
        assert!(!resize_due(MINI_BAR_SIZE + egui::vec2(3.0, 2.0), MINI_BAR_SIZE, Duration::from_millis(300)), "rounding is fine");
        assert!(!resize_due(old, MINI_BAR_SIZE, Duration::from_secs(4)), "the person may resize it later");
        // And back: after Stop the window gets its old size even when the bar's size lingers.
        assert!(resize_due(MINI_BAR_SIZE, old, Duration::from_millis(300)));
    }

    /// Print Screen while Snagster was closed to the tray: the overlay must be made visible
    /// before it goes full screen and takes the keyboard.
    #[test]
    fn the_capture_overlay_is_made_visible_first() {
        use egui::ViewportCommand as C;
        let cmds = super::overlay_show_commands(egui::pos2(0.0, 0.0));
        assert_eq!(cmds.first(), Some(&C::Visible(true)));
        let at = |c: &C| cmds.iter().position(|x| x == c).unwrap();
        assert!(at(&C::Visible(true)) < at(&C::Fullscreen(true)));
        assert!(at(&C::Fullscreen(true)) < at(&C::Focus), "focus last, on the visible full-screen window");
    }

    use super::*;

    #[test]
    fn recording_notes_keep_their_reason() {
        // English here (tests never switch the language): the same words, built from a template.
        assert_eq!(recording_note("Recorded without sound: no speakers or headphones to record from"), "Recorded without sound: no speakers or headphones to record from");
        assert_eq!(recording_note("Something else"), "Something else");
        assert_eq!(recording_note(""), "");
    }

    #[test]
    fn status_texts_built_at_run_time_are_translatable() {
        // Every template the status line fills in is in strings.json, so every pack translates it.
        let keys: Vec<String> = serde_json::from_str(include_str!("../assets/i18n/strings.json")).unwrap();
        for k in [
            "Captured {width} x {height}.",
            "Capture failed: {error}",
            "Saved {name}: {seconds} s, {frames} frames, {width}x{height} ({encoder}). Record > Play last video.",
            "Recorded without sound: {reason}",
            "Open the menu now: Snagster captures it in {seconds} seconds.",
            "Blurred {count} items ({kinds}). Undo brings them back.",
            "Copied.",
        ] {
            assert!(keys.iter().any(|x| x == k), "{k} is missing from strings.json");
        }
        let a = crate::i18n::tf("Captured {width} x {height}.", &[("width", "640"), ("height", "480")]);
        assert_eq!(a, "Captured 640 x 480.");
    }

    /// A window state with one white 200 x 100 picture open; no screen, files or threads.
    pub(crate) fn app() -> SnagApp {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.add_entry("Test".into(), Doc::new(RgbaImage::from_pixel(200, 100, image::Rgba([255, 255, 255, 255]))));
        a
    }

    fn shapes(a: &SnagApp) -> &[Shape] {
        &a.entries[a.current.unwrap()].doc.shapes
    }

    fn text(s: &str) -> Shape {
        Shape::Text {
            pos: pos2(10.0, 10.0),
            text: s.into(),
            color: Color32::RED,
            size: 20.0,
            bg: false,
            shadow: false,
            font: String::new(),
            bold: false,
            italic: false,
            outline: false,
            align: 0,
        }
    }

    fn boxed() -> Shape {
        Shape::Rect { rect: Rect::from_min_max(pos2(20.0, 20.0), pos2(60.0, 60.0)), color: Color32::RED, width: 3.0, fill: false, radius: 0.0, shadow: false }
    }

    #[test]
    fn blank_state_has_nothing_open_and_does_not_persist() {
        let a = SnagApp::blank(Settings::default(), false, None);
        assert!(a.entries.is_empty());
        assert_eq!(a.current, None);
        assert_eq!(a.tool, Tool::Arrow);
        assert!(!a.persist);
        assert!(a.screenshot.is_none());
        assert_eq!(a.x.tab, ribbon::RibbonTab::Home);
    }

    #[test]
    fn add_entry_selects_the_new_picture_and_resets_the_view() {
        let mut a = app();
        a.zoom = Some(3.0);
        a.pan = vec2(5.0, 5.0);
        a.selected = Some(0);
        a.add_entry("Second".into(), Doc::new(RgbaImage::new(10, 10)));
        assert_eq!(a.current, Some(1));
        assert_eq!(a.zoom, None);
        assert_eq!(a.pan, Vec2::ZERO);
        assert_eq!(a.selected, None);
        assert_eq!(a.entry().unwrap().name, "Second");
    }

    #[test]
    fn next_name_counts_up() {
        let mut a = app();
        assert_eq!(a.next_name("Capture"), "Capture 1");
        assert_eq!(a.next_name("Combined"), "Combined 2");
    }

    #[test]
    fn duplicate_copies_picture_and_marks() {
        let mut a = app();
        a.entries[0].doc.shapes.push(boxed());
        a.duplicate();
        assert_eq!(a.entries.len(), 2);
        assert_eq!(a.current, Some(1));
        assert_eq!(a.entries[1].name, "Test copy");
        assert_eq!(a.entries[1].doc.shapes.len(), 1);
    }

    #[test]
    fn remove_current_selects_a_neighbour_and_fixes_the_guide() {
        let mut a = app();
        a.add_entry("B".into(), Doc::new(RgbaImage::new(4, 4)));
        a.add_entry("C".into(), Doc::new(RgbaImage::new(4, 4)));
        a.guide = vec![0, 1, 2];
        a.current = Some(1);
        a.remove_current();
        assert_eq!(a.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Test", "C"]);
        assert_eq!(a.current, Some(1));
        assert_eq!(a.guide, vec![0, 1]);
        a.remove_current();
        assert_eq!(a.current, Some(0));
        a.remove_current();
        assert_eq!(a.current, None);
        a.remove_current(); // nothing left: no panic
    }

    #[test]
    fn push_undo_is_capped_and_clears_redo() {
        let mut e = Entry::new("x".into(), Doc::new(RgbaImage::new(2, 2)));
        e.redo.push(e.doc.clone());
        for _ in 0..UNDO_LIMIT + 5 {
            e.push_undo();
        }
        assert_eq!(e.undo.len(), UNDO_LIMIT);
        assert!(e.redo.is_empty());
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut a = app();
        a.entry().unwrap().push_undo();
        a.entries[0].doc.shapes.push(boxed());
        a.entry().unwrap().push_undo();
        a.entries[0].doc.shapes.push(text("hi"));
        assert_eq!(shapes(&a).len(), 2);
        a.undo();
        assert_eq!(shapes(&a).len(), 1);
        a.undo();
        assert_eq!(shapes(&a).len(), 0);
        a.undo(); // nothing more to undo
        assert_eq!(shapes(&a).len(), 0);
        a.redo();
        a.redo();
        assert_eq!(shapes(&a).len(), 2);
        a.redo(); // nothing more to redo
        assert_eq!(shapes(&a).len(), 2);
        assert_eq!(a.entries[0].undo.len(), 2);
    }

    #[test]
    fn undo_and_redo_without_a_picture_do_nothing() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.undo();
        a.redo();
        assert!(a.entries.is_empty());
    }

    #[test]
    fn finish_draw_makes_the_mark_of_each_drawing_tool() {
        let cases: [(Tool, fn(&Shape) -> bool); 8] = [
            (Tool::Arrow, |s| matches!(s, Shape::Arrow { .. })),
            (Tool::Line, |s| matches!(s, Shape::Line { .. })),
            (Tool::Rect, |s| matches!(s, Shape::Rect { .. })),
            (Tool::Ellipse, |s| matches!(s, Shape::Ellipse { .. })),
            (Tool::Spotlight, |s| matches!(s, Shape::Spotlight { .. })),
            (Tool::Magnify, |s| matches!(s, Shape::Magnify { .. })),
            (Tool::Pen, |s| matches!(s, Shape::Pen { .. })),
            (Tool::Highlight, |s| matches!(s, Shape::Marker { .. })),
        ];
        for (tool, is) in cases {
            let mut a = app();
            a.tool = tool;
            let pts = vec![pos2(20.0, 20.0), pos2(40.0, 30.0), pos2(80.0, 60.0)];
            a.finish_draw(pos2(20.0, 20.0), pos2(80.0, 60.0), pts, false);
            assert_eq!(shapes(&a).len(), 1, "{tool:?}");
            assert!(is(&shapes(&a)[0]), "{tool:?} made {:?}", shapes(&a)[0]);
            assert_eq!(a.entries[0].undo.len(), 1, "{tool:?} is undoable");
        }
    }

    #[test]
    fn the_highlighter_draws_a_box_or_a_freehand_stroke() {
        let mut a = app();
        a.tool = Tool::Highlight;
        a.finish_draw(pos2(10.0, 10.0), pos2(90.0, 30.0), vec![pos2(10.0, 10.0), pos2(90.0, 30.0)], false);
        let Shape::Marker { rect, .. } = &shapes(&a)[0] else { panic!("{:?}", shapes(&a)[0]) };
        assert_eq!(*rect, Rect::from_min_max(pos2(10.0, 10.0), pos2(90.0, 30.0)));
        // A click is no box.
        a.finish_draw(pos2(10.0, 10.0), pos2(11.0, 11.0), vec![pos2(10.0, 10.0)], false);
        assert_eq!(shapes(&a).len(), 1);
        a.settings.highlight_box = false;
        a.finish_draw(pos2(10.0, 50.0), pos2(90.0, 50.0), vec![pos2(10.0, 50.0), pos2(90.0, 50.0)], false);
        assert!(matches!(shapes(&a)[1], Shape::Highlight { .. }));
    }

    #[test]
    fn alt_draws_a_box_around_the_press() {
        let mut a = app();
        a.tool = Tool::Rect;
        a.finish_draw_from(pos2(50.0, 50.0), pos2(70.0, 60.0), vec![], false, true);
        let Shape::Rect { rect, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!(*rect, Rect::from_min_max(pos2(30.0, 40.0), pos2(70.0, 60.0)));
    }

    #[test]
    fn a_new_mark_stays_picked_and_its_handles_adjust_it_with_the_same_tool() {
        let mut a = app();
        a.tool = Tool::Arrow;
        a.finish_draw(pos2(20.0, 20.0), pos2(80.0, 60.0), vec![], false);
        assert_eq!(a.selected, Some(0), "the new arrow is picked");
        // Pressing on its point (still with the arrow tool) takes that handle, not a new arrow.
        a.begin_drag(pos2(79.0, 61.0), 4.0);
        assert!(matches!(a.drag, Drag::Grip { idx: 0, grip: doc::Grip::End, .. }));
        a.drag_grip(pos2(120.0, 30.0), false);
        a.drag_grip(pos2(150.0, 40.0), false);
        let Shape::Arrow { a: tail, b: point, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!((*tail, *point), (pos2(20.0, 20.0), pos2(150.0, 40.0)));
        assert_eq!(shapes(&a).len(), 1, "no second arrow");
        assert_eq!(a.entries[0].undo.len(), 2, "drawing and reshaping are one undo each");
        a.undo();
        let Shape::Arrow { b, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!(*b, pos2(80.0, 60.0));
    }

    #[test]
    fn a_drawing_tool_picks_up_its_own_marks_and_draws_past_others() {
        let mut a = app();
        a.tool = Tool::Rect;
        a.finish_draw(pos2(20.0, 20.0), pos2(80.0, 60.0), vec![], false);
        a.tool = Tool::Arrow;
        a.selected = None;
        // On the box's edge with the arrow tool: a new arrow starts there.
        a.begin_drag(pos2(20.0, 40.0), 4.0);
        assert!(matches!(a.drag, Drag::Draw { .. }));
        a.drag = Drag::None;
        // With the box tool the same press picks the box up to move it.
        a.tool = Tool::Rect;
        a.begin_drag(pos2(20.0, 40.0), 4.0);
        assert!(matches!(a.drag, Drag::Move { idx: 0, .. }));
        assert_eq!(a.selected, Some(0));
        a.drag = Drag::None;
        // A corner of the picked box reshapes it.
        a.begin_drag(pos2(80.0, 60.0), 4.0);
        assert!(matches!(a.drag, Drag::Grip { grip: doc::Grip::Corner(2), .. }));
        a.drag_grip(pos2(100.0, 90.0), false);
        let Shape::Rect { rect, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!(*rect, Rect::from_min_max(pos2(20.0, 20.0), pos2(100.0, 90.0)));
    }

    #[test]
    fn copy_after_drawing_still_copies_the_picture() {
        let mut a = app();
        a.tool = Tool::Arrow;
        a.finish_draw(pos2(20.0, 20.0), pos2(80.0, 60.0), vec![], false);
        assert!(a.selected.is_some());
        assert!(!(a.selected.is_some() && a.tool == Tool::Select), "a fresh mark isn't what Ctrl+C copies");
    }

    #[test]
    fn shift_enter_is_left_for_a_new_line_and_enter_finishes() {
        let key = |shift: bool| egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if shift { Modifiers::SHIFT } else { Modifiers::NONE },
        };
        let mut events = vec![key(true)];
        assert!(!take_finishing_enter(&mut events), "Shift+Enter doesn't finish");
        assert_eq!(events.len(), 1, "the text box still gets it");
        let mut events = vec![key(false)];
        assert!(take_finishing_enter(&mut events));
        assert!(events.is_empty());
    }

    #[test]
    fn a_click_is_not_a_drag() {
        for tool in [Tool::Arrow, Tool::Line, Tool::Rect, Tool::Ellipse, Tool::Spotlight, Tool::Magnify, Tool::Crop, Tool::Blur, Tool::Selection] {
            let mut a = app();
            a.tool = tool;
            a.finish_draw(pos2(20.0, 20.0), pos2(21.0, 21.0), vec![pos2(20.0, 20.0)], false);
            assert!(shapes(&a).is_empty(), "{tool:?}");
            assert!(a.entries[0].undo.is_empty(), "{tool:?}");
            assert_eq!(a.entries[0].doc.image.dimensions(), (200, 100));
        }
    }

    #[test]
    fn shift_makes_squares_and_straight_marker_lines() {
        let mut a = app();
        a.tool = Tool::Rect;
        a.finish_draw(pos2(10.0, 10.0), pos2(70.0, 30.0), vec![], true);
        let Shape::Rect { rect, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!(rect.width(), rect.height());
        a.tool = Tool::Highlight;
        a.settings.highlight_box = false;
        a.finish_draw(pos2(10.0, 10.0), pos2(90.0, 40.0), vec![pos2(10.0, 10.0), pos2(50.0, 30.0), pos2(90.0, 40.0)], true);
        let Shape::Highlight { points, .. } = &shapes(&a)[1] else { panic!() };
        assert_eq!(points, &vec![pos2(10.0, 10.0), pos2(90.0, 10.0)]);
    }

    #[test]
    fn crop_cuts_the_picture_and_goes_back_to_the_arrow() {
        let mut a = app();
        a.tool = Tool::Crop;
        a.zoom = Some(2.0);
        a.finish_draw(pos2(10.0, 10.0), pos2(110.0, 60.0), vec![], false);
        assert_eq!(a.entries[0].doc.image.dimensions(), (100, 50));
        assert_eq!(a.tool, Tool::Arrow);
        assert_eq!(a.zoom, None);
        a.undo();
        assert_eq!(a.entries[0].doc.image.dimensions(), (200, 100));
    }

    #[test]
    fn selection_lifts_a_piece_and_selects_it() {
        let mut a = app();
        a.tool = Tool::Selection;
        a.finish_draw(pos2(10.0, 10.0), pos2(60.0, 60.0), vec![], false);
        assert_eq!(a.tool, Tool::Select);
        assert_eq!(a.selected, Some(shapes(&a).len() - 1));
    }

    #[test]
    fn callout_drag_starts_typing_with_the_tail_at_the_press() {
        let mut a = app();
        a.tool = Tool::Callout;
        a.finish_draw(pos2(10.0, 10.0), pos2(90.0, 60.0), vec![], false);
        let t = a.text_edit.as_ref().unwrap();
        assert!(t.kind == TextKind::Callout { tail: pos2(10.0, 10.0) });
        assert_eq!(t.pos, pos2(90.0, 60.0));
        assert!(t.bg);
        assert!(shapes(&a).is_empty(), "nothing is added before text is typed");
    }

    #[test]
    fn commit_text_adds_trimmed_text_and_skips_empty_text() {
        let mut a = app();
        a.start_text(0, pos2(5.0, 5.0), None);
        a.commit_text();
        assert!(shapes(&a).is_empty());
        assert!(a.entries[0].undo.is_empty());
        a.start_text(0, pos2(5.0, 5.0), None);
        a.text_edit.as_mut().unwrap().text = "Hello  \n".into();
        a.commit_text();
        assert!(a.text_edit.is_none());
        let Shape::Text { text, pos, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!(text, "Hello");
        assert_eq!(*pos, pos2(5.0, 5.0));
        assert_eq!(a.entries[0].undo.len(), 1);
    }

    #[test]
    fn editing_existing_text_changes_it_or_deletes_it_when_emptied() {
        let mut a = app();
        a.entries[0].doc.shapes.push(text("old"));
        a.start_text(0, pos2(0.0, 0.0), Some(0));
        a.text_edit.as_mut().unwrap().text = "new".into();
        a.commit_text();
        let Shape::Text { text, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!(text, "new");
        a.start_text(0, pos2(0.0, 0.0), Some(0));
        a.text_edit.as_mut().unwrap().text.clear();
        a.commit_text();
        assert!(shapes(&a).is_empty());
    }

    #[test]
    fn redo_while_typing_does_not_touch_marks_of_the_redone_picture() {
        // Editing the text mark at index 0, emptied (so committing deletes it)...
        let mut a = app();
        a.entries[0].doc.shapes.push(text("hello"));
        let mut later = a.entries[0].doc.clone();
        later.shapes = vec![boxed()];
        a.entries[0].redo.push(later);
        a.start_text(0, pos2(0.0, 0.0), Some(0));
        a.text_edit.as_mut().unwrap().text.clear();
        // ...then Redo. The edit must land on the picture it was made on, not delete the box
        // that has index 0 in the redone picture.
        a.redo();
        assert!(a.text_edit.is_none(), "the edit was left open over a different picture");
        a.commit_text();
        // The text was removed from its own picture (an edit, so there is nothing left to redo),
        // and undo brings that picture back with the text.
        let e = &a.entries[0];
        assert!(e.doc.shapes.is_empty());
        assert!(e.redo.is_empty());
        assert_eq!(e.undo.len(), 1);
        assert!(matches!(&e.undo[0].shapes[..], [Shape::Text { .. }]));
    }

    #[test]
    fn delete_clear_and_scale_are_undoable() {
        let mut a = app();
        a.entries[0].doc.shapes.extend([boxed(), text("a")]);
        a.selected = Some(0);
        a.scale_selected(2.0);
        let Shape::Rect { rect, .. } = &shapes(&a)[0] else { panic!() };
        assert!(rect.width() > 40.0);
        a.delete_selected();
        assert_eq!(shapes(&a).len(), 1);
        assert_eq!(a.selected, None);
        a.clear_marks();
        assert!(shapes(&a).is_empty());
        assert_eq!(a.entries[0].undo.len(), 3);
        a.clear_marks(); // nothing to clear: no extra undo step
        assert_eq!(a.entries[0].undo.len(), 3);
        a.selected = Some(7);
        a.delete_selected(); // out of range: ignored
        assert_eq!(a.entries[0].undo.len(), 3);
    }

    #[test]
    fn copy_and_paste_mark_puts_an_offset_copy_selected() {
        let mut a = app();
        a.entries[0].doc.shapes.push(boxed());
        a.selected = Some(0);
        a.copy_mark();
        a.paste_mark();
        a.paste_mark();
        assert_eq!(shapes(&a).len(), 3);
        assert_eq!(a.selected, Some(2));
        assert_eq!(a.tool, Tool::Select);
        let mins: Vec<Pos2> = shapes(&a).iter().map(|s| s.bounds().min).collect();
        assert!((mins[1] - mins[0] - vec2(24.0, 24.0)).length() < 0.01);
        assert!((mins[2] - mins[1] - vec2(24.0, 24.0)).length() < 0.01);
        a.selected = Some(0);
        a.cut_mark();
        assert_eq!(shapes(&a).len(), 2);
    }

    #[test]
    fn set_color_changes_the_pen_and_the_selected_mark() {
        let mut a = app();
        a.entries[0].doc.shapes.push(boxed());
        a.selected = Some(0);
        a.set_color(Color32::from_rgb(1, 2, 3));
        assert_eq!(a.settings.color, [1, 2, 3]);
        assert_eq!(a.color(), Color32::from_rgb(1, 2, 3));
        let Shape::Rect { color, .. } = &shapes(&a)[0] else { panic!() };
        assert_eq!(*color, Color32::from_rgb(1, 2, 3));
        assert_eq!(a.entries[0].undo.len(), 1);
    }

    #[test]
    fn pick_finds_the_topmost_mark() {
        let mut a = app();
        a.entries[0].doc.shapes.extend([boxed(), boxed()]);
        assert_eq!(a.pick(pos2(20.0, 40.0)), Some(1));
        assert_eq!(a.pick(pos2(150.0, 90.0)), None);
    }

    #[test]
    fn set_zoom_fits_shows_actual_pixels_and_clamps() {
        let ctx = egui::Context::default();
        let mut a = app();
        a.pan = vec2(9.0, 9.0);
        a.set_zoom(&ctx, Some(0.0));
        assert_eq!(a.zoom, Some(1.0 / ctx.pixels_per_point()));
        assert_eq!(a.pan, Vec2::ZERO);
        a.last_scale = 10.0;
        a.set_zoom(&ctx, Some(4.0));
        assert_eq!(a.zoom, Some(16.0));
        a.last_scale = 0.01;
        a.set_zoom(&ctx, Some(0.5));
        assert_eq!(a.zoom, Some(0.02));
        a.set_zoom(&ctx, None);
        assert_eq!(a.zoom, None);
    }

    #[test]
    fn canvas_maths_fit_centre_and_zoom_about_the_pointer() {
        // A 1000 x 500 picture in a 548 x 548 canvas fits at half size.
        assert_eq!(fit_scale(vec2(548.0, 548.0), vec2(1000.0, 500.0), 1.0), 0.5);
        // Small pictures are never blown up past 1:1 screen pixels...
        assert_eq!(fit_scale(vec2(2000.0, 2000.0), vec2(100.0, 100.0), 2.0), 0.5);
        // ...and tiny canvases never go below 2%.
        assert_eq!(fit_scale(vec2(10.0, 10.0), vec2(100.0, 100.0), 1.0), 0.02);
        let center = pos2(400.0, 300.0);
        let size = vec2(200.0, 100.0);
        let origin = canvas_origin(center, size, 2.0, Vec2::ZERO);
        assert_eq!(origin, pos2(200.0, 200.0));
        assert_eq!(canvas_origin(center, size, 2.0, vec2(10.0, -5.0)), pos2(210.0, 195.0));
        let p = pos2(37.0, 81.0);
        assert_eq!(screen_to_image(image_to_screen(p, origin, 2.0), origin, 2.0), p);
        // Zooming keeps the picture point under the pointer still, and the pan reproduces it.
        let cursor = pos2(260.0, 230.0);
        let under = screen_to_image(cursor, origin, 2.0);
        let (o2, pan) = zoom_about(center, size, origin, 2.0, 5.0, cursor);
        assert!((image_to_screen(under, o2, 5.0) - cursor).length() < 1e-3);
        assert!((canvas_origin(center, size, 5.0, pan) - o2).length() < 1e-3);
    }

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(clean_file_name(r#"a/b\c:d*e?f"g<h>i|j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(clean_file_name("Snag 2 – ok"), "Snag 2 – ok");
        assert_eq!(temp_base("Capture 3 (copy).png"), "Capture_3__copy__png");
        assert_eq!(temp_base("Bild über"), "Bild_über");
    }

    #[test]
    fn rename_without_a_file_renames_the_entry_only() {
        let mut a = app();
        a.rename_current("   ");
        assert_eq!(a.entries[0].name, "Test");
        a.rename_current("  New name ");
        assert_eq!(a.entries[0].name, "New name");
        assert!(a.settings.recent_files.is_empty());
    }

    #[test]
    fn rename_with_a_file_renames_it_on_disk() {
        let dir = std::env::temp_dir().join(format!("snagster-app-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.png");
        std::fs::write(&old, b"x").unwrap();
        let mut a = app();
        a.entries[0].path = Some(old.clone());
        a.rename_current("a:b");
        let want = dir.join("a_b.png");
        assert!(want.exists() && !old.exists());
        assert_eq!(a.entries[0].path.as_deref(), Some(want.as_path()));
        assert_eq!(a.entries[0].name, "a:b");
        assert_eq!(a.settings.recent_files.first(), Some(&want.display().to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_files_are_unique_newest_first_and_capped() {
        let mut a = app();
        for i in 0..20 {
            a.remember_file(std::path::Path::new(&format!("f{i}.png")));
        }
        a.remember_file(std::path::Path::new("f15.png"));
        assert_eq!(a.settings.recent_files.len(), 12);
        assert_eq!(a.settings.recent_files[0], "f15.png");
        assert_eq!(a.settings.recent_files.iter().filter(|f| *f == "f15.png").count(), 1);
        assert_eq!(a.settings.recent_files[1], "f19.png");
    }

    #[test]
    fn every_tool_has_a_unique_label_and_a_unique_working_key() {
        let mut labels = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for t in Tool::ALL {
            assert!(labels.insert(t.label()), "label {} used twice", t.label());
            assert!(keys.insert(t.key()), "key {} used twice", t.key());
            assert!(Key::from_name(t.key()).is_some(), "{:?}'s key {} isn't a key", t, t.key());
            assert!(!t.hint().is_empty());
        }
        // The zoom keys stay free.
        for k in ["0", "1", "+", "=", "-"] {
            assert!(!keys.contains(k));
        }
        // ALL lists every tool exactly once.
        assert_eq!(Tool::ALL.len(), labels.len());
    }

    #[test]
    fn effect_options_follow_the_settings() {
        let mut a = app();
        a.settings.border_width = 7;
        let o = a.effect_options();
        assert_eq!(o.border_width, 7);
        assert!(o.info_text.ends_with("Test"));
    }

    #[test]
    fn the_window_and_the_mcp_server_make_the_same_effect_options() {
        let mut a = app();
        a.settings.border_width = 9;
        a.settings.watermark_text = "Mine".into();
        a.settings.tilt_percent = 33;
        let ui = a.effect_options();
        // What the MCP server's apply_effect builds for a picture of the same name.
        let mcp = a.settings.effect_options(effects::info_line(&["Test"]));
        // (The clock can tick over a minute between the two.)
        assert_eq!(EffectOptions { info_text: String::new(), ..ui.clone() }, EffectOptions { info_text: String::new(), ..mcp });
        assert!(ui.info_text.ends_with("   Test"));
        // No picture open: no name, and no panic on a stale index.
        a.current = Some(99);
        assert!(a.effect_options().info_text.ends_with("   "));
    }

    #[test]
    fn video_recordings_use_the_same_options_as_the_mcp_server() {
        use crate::video::{VideoFormat, VideoOpts};
        let mut a = app();
        a.settings.video_fps = 500;
        a.settings.video_quality = 3;
        a.settings.video_max_s = 7;
        a.settings.gif_cursor = false;
        a.settings.video_folder = std::env::temp_dir().join("snagster-vopts").to_string_lossy().into();
        let rect = (10, 20, 320, 240);
        let (ui, mcp) = (a.video_opts(rect), VideoOpts::from_settings(&a.settings, rect));
        assert_eq!((ui.rect, ui.fps, ui.format, ui.quality, ui.hardware, ui.cursor, ui.max_s, ui.audio), (mcp.rect, mcp.fps, mcp.format, mcp.quality, mcp.hardware, mcp.cursor, mcp.max_s, mcp.audio));
        assert_eq!((ui.fps, ui.max_s, ui.cursor, ui.rect), (120, 7, false, rect));
        assert_eq!(ui.path.parent(), mcp.path.parent());
        assert!(ui.path.file_name().unwrap().to_string_lossy().starts_with("Recording_"));
        // GIF still records a GIF from the window: start_record sends it to start_gif before
        // video options are made.
        a.settings.rec_format = VideoFormat::Gif;
        assert_eq!(a.video_opts(rect).format, VideoFormat::Mp4);
    }

    #[test]
    fn redact_reports_what_it_hid() {
        assert_eq!(redact_status(&["email", "email", "phone number"]), "Blurred 3 items (email, phone number). Undo brings them back.");
    }

    #[test]
    fn saving_uses_the_jpeg_quality_setting_and_writes_word_documents() {
        let d = temp("save-quality");
        let mut a = app();
        a.settings.jpeg_quality = 100;
        a.save_to(0, d.join("hi.jpg"));
        a.settings.jpeg_quality = 5;
        a.save_to(0, d.join("lo.jpg"));
        let size = |n: &str| std::fs::metadata(d.join(n)).unwrap().len();
        assert!(size("lo.jpg") < size("hi.jpg"), "the quality setting is used");
        a.save_to(0, d.join("pic.docx"));
        assert!(std::fs::read(d.join("pic.docx")).unwrap().starts_with(b"PK"), "{}", a.status);
        a.save_in_background(0, d.join("bg.jpg"));
        settle(&mut a);
        assert!(d.join("bg.jpg").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---------- work kept off the window's thread ----------

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-app-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Wait for the job queue and hand its results to the app, as a frame does.
    fn settle(a: &mut SnagApp) {
        let ctx = egui::Context::default();
        assert!(a.jobs.wait_idle(Duration::from_secs(20)), "the jobs never finished");
        for apply in a.jobs.finished(usize::MAX) {
            apply(a, &ctx);
        }
    }

    fn quiet_after_capture(a: &mut SnagApp, folder: &std::path::Path) {
        a.settings.library_folder = folder.join("Library").display().to_string();
        a.settings.save_folder = folder.display().to_string();
        a.settings.after_copy = false;
        a.settings.after_save = false;
        a.settings.after_pin = false;
        a.settings.after_library = true;
        a.pending_panel = None;
        a.pending_preset = None;
    }

    #[test]
    fn a_capture_is_in_recent_at_once_and_in_the_library_soon_after() {
        let dir = temp("capture");
        let mut a = SnagApp::blank(Settings::default(), false, None);
        quiet_after_capture(&mut a, &dir);
        a.lib_loaded = true;
        let t = Instant::now();
        a.finish_capture(RgbaImage::from_pixel(1200, 800, image::Rgba([1, 2, 3, 255])), "Editor", "Notes", "Window", None);
        assert_eq!(a.entries.len(), 1, "Recent has it straight away");
        assert!(a.status.starts_with("Captured 1200 x 800."), "{}", a.status);
        assert!(t.elapsed() < Duration::from_secs(2));
        settle(&mut a);
        assert_eq!(a.lib_items.len(), 1, "the open library got it without a rescan");
        assert_eq!(a.lib_items[0].meta.title, "Editor");
        assert_eq!(a.lib_items[0].meta.app, "Notes");
        assert!(a.lib_items[0].path.exists());
        assert!(a.lib_thumbs.contains_key(&a.lib_items[0].path), "its thumbnail is ready");
        assert!(a.lib_loaded, "no rescan was asked for");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_capture_during_a_library_scan_is_merged_into_it() {
        let dir = temp("scanmerge");
        let mut a = SnagApp::blank(Settings::default(), false, None);
        quiet_after_capture(&mut a, &dir);
        let (_tx, rx) = channel();
        a.lib_scan_rx = Some((a.settings.library_folder.clone(), rx));
        a.finish_capture(RgbaImage::from_pixel(20, 10, image::Rgba([1, 2, 3, 255])), "During", "", "Area", None);
        settle(&mut a);
        assert_eq!(a.lib_added.len(), 1, "kept for the scan's result");
        assert!(a.lib_items.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_library_error_shows_in_the_status_line() {
        let dir = temp("liberr");
        let mut a = SnagApp::blank(Settings::default(), false, None);
        quiet_after_capture(&mut a, &dir);
        // A file where the library folder should be.
        let blocked = dir.join("not a folder");
        std::fs::write(&blocked, b"x").unwrap();
        a.settings.library_folder = blocked.display().to_string();
        a.finish_capture(RgbaImage::from_pixel(20, 10, image::Rgba([1, 2, 3, 255])), "x", "", "Area", None);
        settle(&mut a);
        assert!(a.status.starts_with("Not added to the library"), "{}", a.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_saves_are_written_in_the_background_to_different_files() {
        let dir = temp("quicksave");
        let mut a = app();
        a.settings.save_folder = dir.display().to_string();
        a.settings.name_pattern = "same name".into();
        a.settings.format = crate::settings::ImageFormat::Png;
        a.quick_save();
        a.quick_save();
        assert_eq!(a.saving.len(), 2, "two saves under way, two names");
        settle(&mut a);
        assert!(a.saving.is_empty());
        let p = a.entries[0].path.clone().expect("the picture knows its file");
        assert!(p.exists());
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, 2, "the second save didn't overwrite the first");
        assert!(a.settings.recent_files.first().is_some_and(|f| *f == p.display().to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Exporting many pictures used to encode them all on the window's thread, so the window
    /// stopped answering ("Not responding") for as long as it took. Now the call returns at once
    /// and the files arrive from the job queue, one name each even for equal titles.
    #[test]
    fn exporting_several_pictures_never_holds_up_the_window() {
        let dir = temp("exportmany");
        let mut a = app();
        // Big pictures with the same title: slow to encode, and clashing names.
        for _ in 0..4 {
            a.add_entry("Same".into(), Doc::new(RgbaImage::from_fn(2400, 1600, |x, y| image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8, 255]))));
        }
        let pick = vec![true; a.entries.len()];
        let t = std::time::Instant::now();
        a.export_many(&pick, &dir, crate::settings::ImageFormat::Png);
        let took = t.elapsed();
        assert!(a.status.starts_with("Exporting 5 pictures"), "{}", a.status);
        // A second export meanwhile doesn't pick the names still being written.
        a.export_many(&[false, true], &dir, crate::settings::ImageFormat::Png);
        assert!(took < Duration::from_millis(300), "the window waited {took:?}");
        settle(&mut a);
        assert_eq!(a.status, format!("Exported 1 pictures to {}", dir.display()));
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names.len(), 6, "{names:?}");
        assert!(a.saving.is_empty(), "every name is free again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Combining, templates and the one-page guide used to draw every picture in Recent on the
    /// window's thread (the template menu even did it on every frame it was open).
    #[test]
    fn pictures_made_from_recent_are_made_off_the_window_thread() {
        let mut a = app();
        a.add_entry("Two".into(), Doc::new(RgbaImage::from_pixel(120, 80, image::Rgba([0, 0, 255, 255]))));
        let n = a.entries.len();
        a.combine(true);
        a.export_guide("page");
        a.make_template(Template::BeforeAfter);
        assert_eq!(a.entries.len(), n, "nothing was made on the window's thread");
        assert_eq!(a.status, "Working on it…");
        settle(&mut a);
        assert_eq!(a.entries.len(), n + 3);
        let names: Vec<&str> = a.entries[n..].iter().map(|e| e.name.as_str()).collect();
        assert!(names[0].starts_with("Combined") && names[1].starts_with("Guide") && names[2].starts_with("Before and after"), "{names:?}");
        // Combined top to bottom: as tall as both with a gap, as wide as the wider.
        let (w, h) = a.entries[n].doc.image.dimensions();
        assert!(w >= 200 && h >= 180, "{w} x {h}");
        assert_eq!(a.status, "Made from a template. Mark it up like any picture.");
    }

    /// Opening used to decode the file on the window's thread. Now it arrives from the job
    /// queue, several in the order asked, and a broken file says why.
    #[test]
    fn opened_files_are_read_off_the_window_thread_and_arrive_in_order() {
        let dir = temp("open");
        let mut a = app();
        let (one, two, bad) = (dir.join("one.png"), dir.join("two.png"), dir.join("bad.png"));
        RgbaImage::from_pixel(30, 20, image::Rgba([1, 2, 3, 255])).save(&one).unwrap();
        RgbaImage::from_pixel(40, 10, image::Rgba([4, 5, 6, 255])).save(&two).unwrap();
        std::fs::write(&bad, b"not a picture").unwrap();
        let n = a.entries.len();
        a.open_path(&one);
        a.open_path(&two);
        assert_eq!(a.entries.len(), n, "read on the window's thread");
        assert!(a.status.starts_with("Opening "), "{}", a.status);
        settle(&mut a);
        assert_eq!(a.entries.len(), n + 2);
        assert_eq!((a.entries[n].name.as_str(), a.entries[n + 1].name.as_str()), ("one", "two"));
        assert_eq!(a.entries[n + 1].doc.image.dimensions(), (40, 10));
        assert_eq!(a.entries[n + 1].path.as_deref(), Some(two.as_path()));
        a.open_path(&bad);
        settle(&mut a);
        assert_eq!(a.entries.len(), n + 2);
        assert!(a.status.starts_with("Couldn't open"), "{}", a.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Moving to the Recycle Bin runs on the job queue now: the picture leaves Recent only when
    /// its file went, found by id (another picture may be open by then), and stays when it failed.
    #[test]
    fn a_recycled_picture_leaves_recent_only_once_its_file_has_gone() {
        let mut a = app();
        a.add_entry("Second".into(), Doc::new(RgbaImage::new(8, 8)));
        a.add_entry("Third".into(), Doc::new(RgbaImage::new(8, 8)));
        let (first, third) = (a.entries[0].id, a.entries[2].id);
        a.current = Some(2);
        let p = std::path::Path::new("C:/x/test.png");
        a.recycled(first, p, Err("in use".into()));
        assert_eq!(a.entries.len(), 3, "kept when the file couldn't go");
        assert!(a.status.contains("in use"), "{}", a.status);
        a.recycled(first, p, Ok(()));
        assert_eq!(a.entries.len(), 2);
        assert_eq!(a.current.map(|c| a.entries[c].id), Some(third), "the open picture stays open");
        assert!(a.status.starts_with("Moved "), "{}", a.status);
        a.recycled(first, p, Ok(()));
        assert_eq!(a.entries.len(), 2, "an id that is gone removes nothing");
    }

    #[test]
    fn every_template_lays_out_the_pictures_or_says_too_few() {
        let imgs: Vec<RgbaImage> = (0..3).map(|i| RgbaImage::from_pixel(50 + i * 10, 40, image::Rgba([255, 0, 0, 255]))).collect();
        for t in [Template::SideBySide, Template::Grid2, Template::Grid3, Template::BeforeAfter, Template::Gallery] {
            let out = t.make(&imgs).unwrap_or_else(|| panic!("{t:?} made nothing"));
            assert!(out.width() >= 60 && out.height() >= 40, "{t:?}: {:?}", out.dimensions());
            assert!(!t.name().is_empty());
        }
        let side = Template::SideBySide.make(&imgs).unwrap();
        let grid = Template::Grid2.make(&imgs).unwrap();
        assert!(side.width() > grid.width() && side.height() < grid.height(), "side by side is one row");
        assert!(Template::BeforeAfter.make(&imgs[..1]).is_none(), "before and after needs two");
    }

    #[test]
    fn a_picture_for_another_program_is_written_off_the_window_thread() {
        let dir = temp("picfile");
        let mut a = app();
        let got = std::sync::Arc::new(std::sync::Mutex::new(None));
        let g = got.clone();
        a.with_picture_file(dir.join("share.png"), move |_, p| *g.lock().unwrap() = Some(p));
        settle(&mut a);
        let p = got.lock().unwrap().clone().expect("the program was not started with the file");
        assert!(p.exists());
        // A file that can't be written says so, and nothing is started.
        std::fs::write(dir.join("file"), b"x").unwrap();
        a.with_picture_file(dir.join("file").join("in.png"), |_, _| panic!("started without a file"));
        settle(&mut a);
        assert!(a.status.starts_with("Couldn't prepare the file"), "{}", a.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_background_save_says_so() {
        let dir = temp("savefail");
        let mut a = app();
        let blocked = dir.join("file");
        std::fs::write(&blocked, b"x").unwrap();
        a.save_in_background(0, blocked.join("inside.png"));
        settle(&mut a);
        assert!(a.status.starts_with("Couldn't save"), "{}", a.status);
        assert!(a.entries[0].path.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_finds_its_picture_after_recent_changed() {
        let dir = temp("saveid");
        let mut a = app();
        let p = dir.join("one.png");
        a.save_in_background(0, p.clone());
        // Another picture comes first in the list before the save finishes.
        a.entries.insert(0, Entry::new("Other".into(), Doc::new(RgbaImage::new(4, 4))));
        settle(&mut a);
        assert!(a.entries[0].path.is_none());
        assert_eq!(a.entries[1].path.as_deref(), Some(p.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn gif_editor_with(msgs: Vec<GifMsg>) -> GifEditor {
        let (tx, rx) = channel();
        for m in msgs {
            tx.send(m).unwrap();
        }
        GifEditor::loading_from(PathBuf::from("t.gif"), rx)
    }

    fn frame(w: u32) -> GifMsg {
        GifMsg::Frame(RgbaImage::new(w, 10), 100)
    }

    #[test]
    fn gif_frames_arrive_a_few_per_paint() {
        let ctx = egui::Context::default();
        let mut msgs: Vec<GifMsg> = (0..20).map(|_| frame(30)).collect();
        msgs.push(GifMsg::Done(Ok(())));
        let mut g = gif_editor_with(msgs);
        assert_eq!(g.take_frames(&ctx, 8), GifLoad::Loading);
        assert_eq!(g.frames.len(), 8);
        assert_eq!(g.size, vec2(30.0, 10.0), "sized by the first frame");
        assert_eq!(g.trim, (0, 7), "the kept range follows the frames");
        assert_eq!(g.take_frames(&ctx, 8), GifLoad::Loading);
        assert_eq!(g.take_frames(&ctx, 8), GifLoad::Done);
        assert_eq!(g.frames.len(), 20);
        assert_eq!(g.trim, (0, 19));
        assert!(g.loading.is_none());
        assert_eq!(g.take_frames(&ctx, 8), GifLoad::Done);
    }

    #[test]
    fn a_kept_range_set_while_loading_stays() {
        let ctx = egui::Context::default();
        let mut g = gif_editor_with((0..6).map(|_| frame(5)).collect());
        g.take_frames(&ctx, 3);
        g.trim = (1, 2);
        g.take_frames(&ctx, 3);
        assert_eq!(g.frames.len(), 6);
        assert_eq!(g.trim, (1, 2));
    }

    #[test]
    fn gif_that_cant_be_read_or_is_empty_fails() {
        let ctx = egui::Context::default();
        let mut bad = gif_editor_with(vec![GifMsg::Done(Err("broken".into()))]);
        assert_eq!(bad.take_frames(&ctx, 8), GifLoad::Failed("broken".into()));
        let mut empty = gif_editor_with(vec![GifMsg::Done(Ok(()))]);
        assert_eq!(empty.take_frames(&ctx, 8), GifLoad::Failed(String::new()));
        // The reading thread stopped without saying it was done.
        let (tx, rx) = channel::<GifMsg>();
        drop(tx);
        let mut gone = GifEditor::loading_from(PathBuf::from("t.gif"), rx);
        assert!(matches!(gone.take_frames(&ctx, 8), GifLoad::Failed(_)));
    }

    #[test]
    fn waiting_gif_frames_dont_end_loading() {
        let ctx = egui::Context::default();
        let (_tx, rx) = channel::<GifMsg>();
        let mut g = GifEditor::loading_from(PathBuf::from("t.gif"), rx);
        assert_eq!(g.take_frames(&ctx, 8), GifLoad::Loading);
        assert!(g.loading.is_some());
    }

    #[test]
    fn open_gif_returns_at_once_and_a_missing_file_says_so() {
        let ctx = egui::Context::default();
        let mut a = app();
        let t = Instant::now();
        a.open_gif(&ctx, std::env::temp_dir().join("snagster-no-such-file-xyz.gif"));
        assert!(t.elapsed() < Duration::from_millis(500));
        assert!(a.gif_open && a.gif_editor.is_some());
        let until = Instant::now() + Duration::from_secs(10);
        while a.gif_editor.is_some() && Instant::now() < until {
            a.poll_gif_load(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(a.gif_editor.is_none());
        assert!(a.status.starts_with("Couldn't open the GIF"), "{}", a.status);
    }

    #[test]
    fn unseen_textures_are_let_go_and_made_again() {
        let ctx = egui::Context::default();
        let mut a = app();
        a.entries[0].texture(&ctx);
        let now = Instant::now();
        assert!(!a.entries[0].drop_stale_texture(now, TEXTURE_KEEP), "just drawn");
        assert!(a.entries[0].drop_stale_texture(now + TEXTURE_KEEP, TEXTURE_KEEP));
        assert!(a.entries[0].tex.is_none());
        a.entries[0].texture(&ctx);
        assert!(a.entries[0].tex.is_some(), "made again when shown");
    }

    #[test]
    fn the_shown_picture_keeps_its_texture() {
        let ctx = egui::Context::default();
        let mut a = app();
        a.entries[0].texture(&ctx);
        let later = Instant::now() + TEXTURE_KEEP * 2;
        a.sweep_textures(later);
        assert!(a.entries[0].tex.is_some(), "the current picture's texture stays");
        a.add_entry("Second".into(), Doc::new(RgbaImage::new(4, 4)));
        a.sweep_textures(later + Duration::from_secs(1));
        assert!(a.entries[0].tex.is_some(), "sweeps are a few seconds apart");
        a.sweep_textures(later + Duration::from_secs(10));
        assert!(a.entries[0].tex.is_none(), "the one not shown for long goes");
    }

    #[test]
    fn entries_have_their_own_ids() {
        let a = Entry::new("a".into(), Doc::new(RgbaImage::new(1, 1)));
        let b = Entry::new("a".into(), Doc::new(RgbaImage::new(1, 1)));
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn stamps_the_font_cant_draw_are_remembered() {
        let ctx = egui::Context::default();
        let mut a = app();
        // A private-use character no emoji font has.
        assert!(!a.stamp_ready('\u{F8FF}', 32));
        let first = a.stamp_texture(&ctx, '\u{F8FF}', 32, [0, 0, 0, 255]);
        assert!(a.stamp_ready('\u{F8FF}', 32), "tried once, not every frame");
        assert_eq!(a.stamp_texture(&ctx, '\u{F8FF}', 32, [0, 0, 0, 255]), first);
        assert!(a.stamp_texture(&ctx, POINTER_KEY, 32, [0, 0, 0, 255]).is_some());
        assert!(a.stamp_ready(POINTER_KEY, 30), "sizes are rounded the same way");
    }

    #[test]
    fn the_slow_frame_line_names_what_is_open() {
        let mut a = app();
        assert!(a.open_parts().is_empty());
        a.library_open = true;
        a.tool = Tool::Stamp;
        let parts = a.open_parts();
        assert!(parts.contains(&"Library") && parts.contains(&"stamp picker"), "{parts:?}");
        // Not persisting (tests): nothing is written to the real crash.log.
        a.watchdog("ui", Duration::from_secs(3));
    }
}
