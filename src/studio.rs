//! Studio mode: a broadcast-style recorder. Scenes hold sources (a screen, a window, a screen
//! area, a picture, text, a colour) laid out on a canvas; the engine composites the current
//! scene many times a second for the live preview and, while recording, hands every frame to
//! the video recorder (hardware encoded on Windows) together with the mixed sound.

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a source shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SourceKind {
    /// A whole screen (by index in the screen list).
    Display { index: usize },
    /// A window, found by part of its title.
    Window { title: String },
    /// A rectangle of the desktop.
    Area { x: i32, y: i32, w: u32, h: u32 },
    /// A picture file.
    Image { path: String },
    /// Text.
    Text { text: String, size: f32, color: [u8; 4] },
    /// A solid colour.
    Color { color: [u8; 4] },
}

impl SourceKind {
    pub fn label(&self) -> &'static str {
        match self {
            SourceKind::Display { .. } => "Screen",
            SourceKind::Window { .. } => "Window",
            SourceKind::Area { .. } => "Screen area",
            SourceKind::Image { .. } => "Picture",
            SourceKind::Text { .. } => "Text",
            SourceKind::Color { .. } => "Color",
        }
    }

    pub fn icon(&self) -> &'static str {
        use crate::look::ph;
        match self {
            SourceKind::Display { .. } => ph::MONITOR,
            SourceKind::Window { .. } => ph::APP_WINDOW,
            SourceKind::Area { .. } => ph::SELECTION,
            SourceKind::Image { .. } => ph::IMAGE,
            SourceKind::Text { .. } => ph::TEXT_T,
            SourceKind::Color { .. } => ph::PAINT_BUCKET,
        }
    }
}

/// A source placed on the canvas.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub kind: SourceKind,
    /// Position and size on the canvas, in canvas pixels.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub visible: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scene {
    pub name: String,
    /// Bottom first: later items are drawn over earlier ones.
    pub items: Vec<Item>,
    /// This scene's own sound (besides `StudioSettings::shared_sounds`).
    #[serde(default)]
    pub sounds: Vec<SceneSound>,
}

/// At most this many sound inputs in a scene, the shared ones included: enough for a game, a
/// second program, two microphones and music, and a mixer that still fits on one screen.
pub const MAX_SOUNDS: usize = 6;

/// One sound input of a scene, with its mixer settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SceneSound {
    pub input: crate::audio::SoundInput,
    /// What the mixer calls it (the microphone's or the file's name).
    pub name: String,
    /// 0..2.
    pub volume: f32,
    pub muted: bool,
}

impl SceneSound {
    pub fn new(input: crate::audio::SoundInput, name: &str) -> SceneSound {
        SceneSound { input, name: name.to_string(), volume: 1.0, muted: false }
    }
}

/// Studio's saved setup.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct StudioSettings {
    pub scenes: Vec<Scene>,
    pub current: usize,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Sound in every scene (usually the microphone), set up once.
    #[serde(default)]
    pub shared_sounds: Vec<SceneSound>,
    /// The scenes' sound has been set up (settings from before per-scene sound are moved over
    /// once by `ready_sounds`).
    #[serde(default)]
    pub sounds_ready: bool,
    /// The old two-slider mixer: read from older settings by `ready_sounds`, no longer used.
    pub system_volume: f32,
    pub system_muted: bool,
    pub mic_volume: f32,
    pub mic_muted: bool,
    /// Live ink: fade after `ink_fade_s` seconds or stay until cleared.
    pub ink_mode: crate::ink::InkMode,
    /// Seconds ink holds before it fades.
    pub ink_fade_s: f32,
    /// The ink colour.
    pub ink_color: [u8; 3],
    /// Pen width in desktop pixels.
    pub ink_width: f32,
}

impl Default for StudioSettings {
    fn default() -> Self {
        StudioSettings {
            scenes: vec![Scene {
                name: "Scene 1".into(),
                items: vec![Item { name: "Screen 1".into(), kind: SourceKind::Display { index: 0 }, x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, visible: true }],
                sounds: Vec::new(),
            }],
            current: 0,
            width: 1920,
            height: 1080,
            fps: 30,
            shared_sounds: vec![SceneSound::new(crate::audio::SoundInput::Computer, "Computer sound")],
            sounds_ready: true,
            system_volume: 1.0,
            system_muted: false,
            mic_volume: 1.0,
            mic_muted: false,
            ink_mode: crate::ink::InkMode::Fade,
            ink_fade_s: 3.0,
            ink_color: [230, 40, 40],
            ink_width: 6.0,
        }
    }
}

impl StudioSettings {
    /// The pen these settings give new ink.
    pub fn ink_pen(&self) -> crate::ink::Pen {
        crate::ink::Pen { color: [self.ink_color[0], self.ink_color[1], self.ink_color[2], 255], width: self.ink_width.clamp(1.0, 64.0), mode: self.ink_mode, hold_s: self.ink_fade_s.clamp(0.5, 600.0) }
    }

    pub fn scene(&self) -> Option<&Scene> {
        self.scenes.get(self.current).or(self.scenes.first())
    }

    /// Settings from before per-scene sound: what the old Sound choice recorded becomes the
    /// shared sound, with the old sliders' volumes. Done once.
    pub fn ready_sounds(&mut self, old: crate::audio::AudioSource, old_mic: &str) {
        if self.sounds_ready {
            return;
        }
        use crate::audio::SoundInput;
        self.shared_sounds.clear();
        if old.system() {
            self.shared_sounds.push(SceneSound { input: SoundInput::Computer, name: "Computer sound".into(), volume: self.system_volume, muted: self.system_muted });
        }
        if old.mic() {
            self.shared_sounds.push(SceneSound { input: SoundInput::Microphone { id: old_mic.to_string() }, name: "Microphone".into(), volume: self.mic_volume, muted: self.mic_muted });
        }
        self.sounds_ready = true;
        self.tidy_sounds();
    }

    /// Keep the sound lists sane: no input twice in a scene (a scene's own copy of a shared
    /// input goes), at most `MAX_SOUNDS` in each, volumes 0..2.
    pub fn tidy_sounds(&mut self) {
        let mut shared: Vec<SceneSound> = Vec::new();
        for s in self.shared_sounds.drain(..) {
            if !shared.iter().any(|x| x.input == s.input) {
                shared.push(s);
            }
        }
        shared.truncate(MAX_SOUNDS);
        for s in &mut shared {
            s.volume = s.volume.clamp(0.0, 2.0);
        }
        for scene in &mut self.scenes {
            let mut own: Vec<SceneSound> = Vec::new();
            for s in scene.sounds.drain(..) {
                if !shared.iter().chain(own.iter()).any(|x| x.input == s.input) {
                    own.push(s);
                }
            }
            own.truncate(MAX_SOUNDS - shared.len());
            for s in &mut own {
                s.volume = s.volume.clamp(0.0, 2.0);
            }
            scene.sounds = own;
        }
        self.shared_sounds = shared;
    }

    /// Scene `i`'s sound: the shared inputs, then its own.
    pub fn scene_sounds(&self, i: usize) -> Vec<&SceneSound> {
        self.shared_sounds.iter().chain(self.scenes.get(i).map(|s| s.sounds.iter()).into_iter().flatten()).collect()
    }

    /// Whether scene `i` has room for another sound input.
    pub fn room_for_sound(&self, i: usize) -> bool {
        self.scene_sounds(i).len() < MAX_SOUNDS
    }

    /// Every input any scene uses (each once): what a recording or stream of Studio captures.
    pub fn sound_inputs(&self) -> Vec<crate::audio::SoundInput> {
        let mut out: Vec<crate::audio::SoundInput> = Vec::new();
        for s in self.shared_sounds.iter().chain(self.scenes.iter().flat_map(|s| s.sounds.iter())) {
            if !out.contains(&s.input) {
                out.push(s.input.clone());
            }
        }
        out
    }

    /// The current scene's mix: (input, volume, muted) of each of its sounds.
    pub fn mix(&self) -> Vec<(crate::audio::SoundInput, f32, bool)> {
        let c = self.current.min(self.scenes.len().saturating_sub(1));
        self.scene_sounds(c).into_iter().map(|s| (s.input.clone(), s.volume, s.muted)).collect()
    }

    /// Hand the current scene's mix to the sound mixer (as the Studio window does every
    /// frame): a recording or stream of Studio hears this scene's sound, fading on a change.
    pub fn apply_mix(&self) {
        crate::audio::set_scene_mix(self.mix());
    }

    /// Add an empty scene (named "Scene N" unless given a name); returns its number.
    pub fn add_scene(&mut self, name: Option<&str>) -> usize {
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| format!("Scene {}", self.scenes.len() + 1));
        self.scenes.push(Scene { name, items: Vec::new(), sounds: Vec::new() });
        self.scenes.len() - 1
    }

    /// Remove scene `i`; the last scene stays. The current scene keeps pointing at the same one
    /// (or the one before when it was removed).
    pub fn remove_scene(&mut self, i: usize) -> Result<Scene, String> {
        if i >= self.scenes.len() {
            return Err(format!("there is no scene {i}"));
        }
        if self.scenes.len() == 1 {
            return Err("Studio needs at least one scene".into());
        }
        let gone = self.scenes.remove(i);
        if self.current > i || (self.current == i && i > 0) {
            self.current -= 1;
        }
        self.current = self.current.min(self.scenes.len() - 1);
        Ok(gone)
    }

    /// Make scene `i` the current one.
    pub fn set_current(&mut self, i: usize) -> Result<(), String> {
        if i >= self.scenes.len() {
            return Err(format!("there is no scene {i}"));
        }
        self.current = i;
        Ok(())
    }
}

/// The options a Studio recording gets from the settings (what the Studio window uses): the
/// canvas size, Studio's frame rate, quality, encoder and sound, and a new file in the
/// recordings folder. A GIF format setting records MP4, as the window does.
pub fn video_opts(s: &crate::settings::Settings) -> crate::video::VideoOpts {
    let format = s.rec_format.video_or_mp4();
    let path = crate::video::recording_path(&s.video_folder, "Studio", format.ext());
    let mut opts = crate::video::VideoOpts::screen((0, 0, s.studio.width, s.studio.height), s.studio.fps.clamp(1, 120), format, path);
    opts.quality = s.video_quality.clamp(1, 4);
    opts.hardware = s.video_hardware;
    opts.audio = s.audio_source;
    opts.mic = s.audio_mic.clone();
    // Studio records its scenes' sound, mixed by the scene shown.
    opts.sound = Some(s.studio.sound_inputs());
    opts
}

/// Draws scenes into frames. Keeps pictures and window positions between frames.
#[derive(Default)]
pub struct Compositor {
    pictures: std::collections::HashMap<String, Option<RgbaImage>>,
    windows: std::collections::HashMap<String, (Instant, Option<(i32, i32, u32, u32)>)>,
    screens: Option<(Instant, Vec<crate::capture::MonitorInfo>)>,
    texts: std::collections::HashMap<String, RgbaImage>,
    /// Wayland: one screen share per live area (the desktop asks the person once), instead of
    /// a desktop screenshot per frame. A share that couldn't start is retried after a while.
    feeds: std::collections::HashMap<(i32, i32, u32, u32), Result<crate::video::ScreenFeed, Instant>>,
    /// Where the last frame showed parts of the desktop (for live ink), bottom first.
    pub placed: Vec<crate::ink::Placed>,
    /// Live ink, drawn once per change and kept (see `ink::Layer`).
    ink_layers: crate::ink::Layers,
}

/// How long a screen share that couldn't start (the person said no, or the desktop has none)
/// waits before Studio asks again.
const FEED_RETRY: Duration = Duration::from_secs(30);

/// Whether a failed screen share should be asked for again.
fn feed_retry_due(failed_at: Instant, now: Instant) -> bool {
    now.duration_since(failed_at) >= FEED_RETRY
}

impl Compositor {
    /// The newest picture of the screen share for `rect` (started on first use).
    fn feed_picture(&mut self, rect: (i32, i32, u32, u32)) -> Option<RgbaImage> {
        let now = Instant::now();
        let due = match self.feeds.get(&rect) {
            None => true,
            Some(Err(at)) => feed_retry_due(*at, now),
            Some(Ok(_)) => false,
        };
        if due {
            let o = crate::video::FeedOptions { fps: 30, cursor: true, fit: None, every_frame: false };
            self.feeds.insert(rect, crate::video::ScreenFeed::start(rect, o).map_err(|_| now));
        }
        let Some(Ok(feed)) = self.feeds.get(&rect) else { return None };
        if let Some(e) = feed.error()
            && feed.frames() == 0
            && e.contains("said no")
        {
            self.feeds.insert(rect, Err(now));
            return None;
        }
        feed.with_latest(|f| (f.width > 0 && f.height > 0).then(|| RgbaImage::from_raw(f.width, f.height, f.rgba.clone())).flatten())
    }

    fn screen_rect(&mut self, index: usize) -> Option<(i32, i32, u32, u32)> {
        let fresh = self.screens.as_ref().is_some_and(|(t, _)| t.elapsed() < Duration::from_secs(3));
        if !fresh {
            self.screens = Some((Instant::now(), crate::capture::monitors()));
        }
        let list = &self.screens.as_ref()?.1;
        list.get(index).or(list.first()).map(|m| (m.x, m.y, m.width, m.height))
    }

    fn window_rect(&mut self, title: &str) -> Option<(i32, i32, u32, u32)> {
        if let Some((t, r)) = self.windows.get(title)
            && t.elapsed() < Duration::from_millis(700)
        {
            return *r;
        }
        let low = title.to_lowercase();
        let r = crate::capture::list_windows().into_iter().find(|w| w.label.to_lowercase().contains(&low)).map(|w| (w.x, w.y, w.width, w.height));
        self.windows.insert(title.to_string(), (Instant::now(), r));
        r
    }

    fn text_image(&mut self, text: &str, size: f32, color: [u8; 4]) -> RgbaImage {
        let key = format!("{text}\u{1}{size}\u{1}{color:?}");
        if let Some(img) = self.texts.get(&key) {
            return img.clone();
        }
        let img = crate::render::text_image(text, size, color);
        if self.texts.len() > 64 {
            self.texts.clear();
        }
        self.texts.insert(key, img.clone());
        img
    }

    /// Draw `scene` into `out` (RGBA, the canvas size).
    #[cfg(test)]
    pub fn draw(&mut self, scene: &Scene, out: &mut RgbaImage) {
        self.draw_with_ink(scene, out, &crate::ink::Snapshot::default());
    }

    /// Draw `scene` with live ink: ink drawn on the screen goes on each source showing that
    /// part of the desktop (right after it, so sources above cover it as they cover the
    /// screen), ink drawn on the preview on top of everything.
    pub fn draw_with_ink(&mut self, scene: &Scene, out: &mut RgbaImage, ink: &crate::ink::Snapshot) {
        self.placed.clear();
        for p in out.pixels_mut() {
            *p = image::Rgba([0, 0, 0, 255]);
        }
        for item in scene.items.iter().filter(|i| i.visible) {
            let (dx, dy, dw, dh) = (item.x.round() as i64, item.y.round() as i64, item.w.round().max(1.0) as u32, item.h.round().max(1.0) as u32);
            match &item.kind {
                SourceKind::Color { color } => fill(out, dx, dy, dw, dh, *color),
                SourceKind::Image { path } => {
                    let pic = self.pictures.entry(path.clone()).or_insert_with(|| image::open(path).ok().map(|i| i.to_rgba8())).clone();
                    if let Some(p) = pic {
                        blit_scaled(out, &p, dx, dy, dw, dh, true);
                    }
                }
                SourceKind::Text { text, size, color } => {
                    let img = self.text_image(text, *size, *color);
                    let (w, h) = img.dimensions();
                    blit_scaled(out, &img, dx, dy, w.min(dw.max(1)), h.min(dh.max(1)), true);
                }
                live => {
                    let rect = match live {
                        SourceKind::Display { index } => self.screen_rect(*index),
                        SourceKind::Window { title } => self.window_rect(title),
                        SourceKind::Area { x, y, w, h } => Some((*x, *y, *w, *h)),
                        _ => None,
                    };
                    let shown = rect.filter(|r| r.2 > 0 && r.3 > 0).map(|desk| crate::ink::Placed { desk, canvas: (dx, dy, dw, dh) });
                    if let Some((x, y, w, h)) = rect
                        && w > 0
                        && h > 0
                        && crate::capture::desktop_screenshots_only()
                    {
                        // Wayland: the desktop's screen sharing, never a screenshot per frame.
                        if let Some(img) = self.feed_picture((x, y, w, h)) {
                            blit_scaled(out, &img, dx, dy, dw, dh, false);
                        }
                    } else if let Some((x, y, w, h)) = rect
                        && w > 0
                        && h > 0
                        && let Ok(mut img) = crate::capture::grab_rect(x, y, w, h)
                    {
                        if let Some(c) = crate::capture::cursor_pos() {
                            crate::capture::stamp_pointer(&mut img, ((c.0 - x) as f32, (c.1 - y) as f32));
                        }
                        blit_scaled(out, &img, dx, dy, dw, dh, false);
                    }
                    if let Some(p) = shown {
                        // Where the screen copy already holds the overlay's ink, not twice.
                        if !crate::ink_overlay::overlay_in_captures() {
                            self.ink_layers.draw_placed(out, ink, &p);
                        }
                        self.placed.push(p);
                    }
                }
            }
        }
        self.ink_layers.draw_canvas(out, ink);
        self.ink_layers.end_frame();
    }
}

fn fill(out: &mut RgbaImage, x: i64, y: i64, w: u32, h: u32, c: [u8; 4]) {
    let (ow, oh) = (out.width() as i64, out.height() as i64);
    let a = c[3] as u32;
    for yy in y.max(0)..(y + h as i64).min(oh) {
        for xx in x.max(0)..(x + w as i64).min(ow) {
            let p = out.get_pixel_mut(xx as u32, yy as u32);
            for k in 0..3 {
                p.0[k] = ((c[k] as u32 * a + p.0[k] as u32 * (255 - a)) / 255) as u8;
            }
        }
    }
}

/// Draw `src` scaled to `w` × `h` at (x, y), bilinear, clipped to `out`. `blend` honours alpha.
pub fn blit_scaled(out: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64, w: u32, h: u32, blend: bool) {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 || w == 0 || h == 0 {
        return;
    }
    let (ow, oh) = (out.width() as i64, out.height() as i64);
    let x0 = x.max(0);
    let x1 = (x + w as i64).min(ow);
    let y0 = y.max(0);
    let y1 = (y + h as i64).min(oh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let same = sw == w && sh == h;
    // Source x per output column, in 16.16 fixed point.
    let fx = ((sw as u64) << 16) / w as u64;
    let fy = ((sh as u64) << 16) / h as u64;
    let cols: Vec<(u32, u32, u32)> = (x0..x1)
        .map(|ox| {
            let sx = (((ox - x) as u64 * fx) + fx / 2).saturating_sub(1 << 15);
            let i = ((sx >> 16) as u32).min(sw - 1);
            (i, (i + 1).min(sw - 1), ((sx & 0xffff) >> 8) as u32)
        })
        .collect();
    let s = src.as_raw();
    let stride = (sw * 4) as usize;
    let ow_u = out.width() as usize;
    let o = &mut **out;
    for oy in y0..y1 {
        let (r0, r1, wy) = if same {
            ((oy - y) as u32, (oy - y) as u32, 0)
        } else {
            let sy = (((oy - y) as u64 * fy) + fy / 2).saturating_sub(1 << 15);
            let i = ((sy >> 16) as u32).min(sh - 1);
            (i, (i + 1).min(sh - 1), ((sy & 0xffff) >> 8) as u32)
        };
        let row0 = &s[r0 as usize * stride..r0 as usize * stride + stride];
        let row1 = &s[r1 as usize * stride..r1 as usize * stride + stride];
        let base = oy as usize * ow_u * 4;
        for (k, &(c0, c1, wx)) in cols.iter().enumerate() {
            let di = base + (x0 as usize + k) * 4;
            let mut px = [0u32; 4];
            if same {
                let si = ((x0 - x) as usize + k) * 4;
                for (c, v) in px.iter_mut().enumerate() {
                    *v = row0[si + c] as u32;
                }
            } else {
                let (a, b) = (c0 as usize * 4, c1 as usize * 4);
                for (c, v) in px.iter_mut().enumerate() {
                    let top = row0[a + c] as u32 * (256 - wx) + row0[b + c] as u32 * wx;
                    let bot = row1[a + c] as u32 * (256 - wx) + row1[b + c] as u32 * wx;
                    *v = (top * (256 - wy) + bot * wy) >> 16;
                }
            }
            if blend && px[3] < 255 {
                let a = px[3];
                for c in 0..3 {
                    o[di + c] = ((px[c] * a + o[di + c] as u32 * (255 - a)) / 255) as u8;
                }
            } else {
                o[di] = px[0] as u8;
                o[di + 1] = px[1] as u8;
                o[di + 2] = px[2] as u8;
                o[di + 3] = 255;
            }
        }
    }
}

/// What the engine shares with the window.
pub struct Shared {
    /// The scene to draw (the window keeps it current).
    pub scene: Mutex<Scene>,
    /// The newest full frame (RGBA).
    pub frame: Mutex<Option<Arc<RgbaImage>>>,
    /// Counts frames, so the window knows when to refresh its preview.
    pub frame_no: AtomicU64,
    pub stop: AtomicBool,
    /// Frames per second while recording (preview runs at 15).
    pub recording_fps: Mutex<Option<u32>>,
    /// Milliseconds the last frame took to draw.
    pub draw_ms: AtomicU64,
    /// Where the last frame showed parts of the desktop (the preview maps ink drawn on it).
    pub placed: Mutex<Vec<crate::ink::Placed>>,
    /// Draw the program's live ink into the frames (off in unit tests, which share it).
    pub ink: AtomicBool,
}

/// The background thread that keeps drawing the scene.
pub struct Engine {
    pub shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    pub fn start(scene: Scene, width: u32, height: u32, ctx: Option<eframe::egui::Context>) -> Engine {
        let shared = Arc::new(Shared {
            scene: Mutex::new(scene),
            frame: Mutex::new(None),
            frame_no: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            recording_fps: Mutex::new(None),
            draw_ms: AtomicU64::new(0),
            placed: Mutex::new(Vec::new()),
            ink: AtomicBool::new(!cfg!(test)),
        });
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("snagster-studio".into())
            .spawn(move || {
                crate::capture::background_thread(false);
                let mut comp = Compositor::default();
                let mut canvas = RgbaImage::new(width.max(16), height.max(16));
                let mut next = Instant::now();
                while !sh.stop.load(Ordering::Relaxed) {
                    let fps = sh.recording_fps.lock().ok().and_then(|f| *f).unwrap_or(15).clamp(1, 120);
                    let scene = sh.scene.lock().map(|s| s.clone()).ok();
                    let t = Instant::now();
                    if let Some(scene) = scene {
                        let ink = if sh.ink.load(Ordering::Relaxed) { crate::ink::with(|i| i.snapshot(Instant::now())) } else { crate::ink::Snapshot::default() };
                        comp.draw_with_ink(&scene, &mut canvas, &ink);
                        if let Ok(mut p) = sh.placed.lock() {
                            p.clone_from(&comp.placed);
                        }
                    }
                    sh.draw_ms.store(t.elapsed().as_millis() as u64, Ordering::Relaxed);
                    if let Ok(mut f) = sh.frame.lock() {
                        *f = Some(Arc::new(canvas.clone()));
                    }
                    sh.frame_no.fetch_add(1, Ordering::Relaxed);
                    if let Some(c) = &ctx {
                        c.request_repaint();
                    }
                    next += Duration::from_nanos(1_000_000_000 / fps as u64);
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now;
                    }
                }
            })
            .ok();
        Engine { shared, thread }
    }

    pub fn latest(&self) -> Option<Arc<RgbaImage>> {
        self.shared.frame.lock().ok().and_then(|f| f.clone())
    }

    /// A frame feed for the video recorder: the newest frame, as BGRA.
    pub fn feed(&self) -> crate::video::FrameFeed {
        let sh = self.shared.clone();
        Arc::new(move |buf: &mut [u8]| {
            if sh.stop.load(Ordering::Relaxed) {
                return false;
            }
            let frame = sh.frame.lock().ok().and_then(|f| f.clone());
            if let Some(f) = frame {
                let src = f.as_raw();
                let n = buf.len().min(src.len());
                for (d, s) in buf[..n].chunks_exact_mut(4).zip(src[..n].chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
            true
        })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Record a scene for `seconds` without a window (for AI apps and the command line), with
/// Studio's mixer. Setting `stop` ends it early (the file is kept).
pub fn record_headless(settings: &StudioSettings, scene: Option<usize>, seconds: u32, mut opts: crate::video::VideoOpts, stop: &AtomicBool) -> crate::video::VideoDone {
    let mut s = settings.clone();
    if let Some(i) = scene {
        s.current = i;
    }
    if opts.audio != crate::audio::AudioSource::None {
        s.apply_mix();
    }
    let scene = s.scene().cloned().unwrap_or_else(|| StudioSettings::default().scenes[0].clone());
    let engine = Engine::start(scene, s.width, s.height, None);
    if let Ok(mut f) = engine.shared.recording_fps.lock() {
        *f = Some(opts.fps);
    }
    // The first frame, so the video doesn't start black.
    let t = Instant::now();
    while engine.latest().is_none() && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    opts.rect = (0, 0, s.width, s.height);
    opts.frames = Some(engine.feed());
    opts.max_s = seconds.max(1);
    let pause = AtomicBool::new(false);
    crate::video::record(&opts, stop, &pause)
}

#[cfg(test)]
mod tests {
    /// A screen share the person refused is asked for again only after a while, not every frame
    /// (on Wayland each ask is a dialog).
    #[test]
    fn a_refused_screen_share_is_asked_again_only_after_a_while() {
        let t = Instant::now();
        assert!(!super::feed_retry_due(t, t + Duration::from_secs(1)));
        assert!(!super::feed_retry_due(t, t + super::FEED_RETRY - Duration::from_millis(1)));
        assert!(super::feed_retry_due(t, t + super::FEED_RETRY));
    }

    use super::*;

    #[test]
    fn same_size_blit_copies_exactly() {
        let mut src = RgbaImage::new(4, 3);
        for (i, p) in src.pixels_mut().enumerate() {
            *p = image::Rgba([i as u8 * 10, 50, 200, 255]);
        }
        let mut out = RgbaImage::new(10, 10);
        blit_scaled(&mut out, &src, 2, 3, 4, 3, false);
        assert_eq!(out.get_pixel(2, 3), src.get_pixel(0, 0));
        assert_eq!(out.get_pixel(5, 5), src.get_pixel(3, 2));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 0]);
    }

    #[test]
    fn scaled_blit_fills_its_box_and_clips_at_the_edges() {
        let src = RgbaImage::from_pixel(8, 8, image::Rgba([10, 200, 30, 255]));
        let mut out = RgbaImage::new(20, 20);
        blit_scaled(&mut out, &src, -5, 14, 16, 16, false);
        assert_eq!(out.get_pixel(0, 19).0, [10, 200, 30, 255]);
        assert_eq!(out.get_pixel(10, 19).0, [10, 200, 30, 255]);
        assert_eq!(out.get_pixel(11, 19).0, [0, 0, 0, 0]);
        assert_eq!(out.get_pixel(0, 13).0, [0, 0, 0, 0]);
    }

    #[test]
    fn a_colour_scene_draws_without_a_screen() {
        let scene = Scene {
            sounds: Vec::new(),
            name: "t".into(),
            items: vec![
                Item { name: "bg".into(), kind: SourceKind::Color { color: [255, 0, 0, 255] }, x: 0.0, y: 0.0, w: 32.0, h: 32.0, visible: true },
                Item { name: "hidden".into(), kind: SourceKind::Color { color: [0, 0, 255, 255] }, x: 0.0, y: 0.0, w: 32.0, h: 32.0, visible: false },
            ],
        };
        let mut out = RgbaImage::new(32, 32);
        Compositor::default().draw(&scene, &mut out);
        assert_eq!(out.get_pixel(16, 16).0, [255, 0, 0, 255]);
    }

    #[test]
    fn the_mixer_comes_from_the_current_scenes_sound() {
        use crate::audio::SoundInput;
        let mic = SoundInput::Microphone { id: String::new() };
        let music = SoundInput::File { path: "/m/intro.wav".into() };
        let mut s = StudioSettings { shared_sounds: vec![SceneSound { volume: 0.8, ..SceneSound::new(mic.clone(), "Mic") }], ..Default::default() };
        s.add_scene(Some("Intro"));
        s.scenes[1].sounds.push(SceneSound { muted: true, ..SceneSound::new(music.clone(), "intro.wav") });
        s.current = 0;
        assert_eq!(s.mix(), vec![(mic.clone(), 0.8, false)], "scene 1: the shared microphone only");
        s.current = 1;
        assert_eq!(s.mix(), vec![(mic.clone(), 0.8, false), (music.clone(), 1.0, true)], "scene 2: shared first, then its own");
        assert_eq!(s.sound_inputs(), vec![mic, music], "every input any scene uses, once");
        s.current = 99;
        assert_eq!(s.mix().len(), 2, "a current scene past the end is the last one");
    }

    /// Settings from before per-scene sound: the old Sound choice and sliders become the shared
    /// sound, once.
    #[test]
    fn old_studio_sound_settings_move_to_the_shared_sound() {
        use crate::audio::{AudioSource, SoundInput};
        let old: StudioSettings = serde_json::from_str(r#"{"system_volume":0.5,"system_muted":true,"mic_volume":1.5,"mic_muted":false,"scenes":[{"name":"A","items":[]}]}"#).unwrap();
        assert!(!old.sounds_ready && old.scenes[0].sounds.is_empty());
        let mut s = old.clone();
        s.ready_sounds(AudioSource::Both, "usb-mic");
        assert_eq!(s.shared_sounds.len(), 2);
        assert_eq!((s.shared_sounds[0].input.clone(), s.shared_sounds[0].volume, s.shared_sounds[0].muted), (SoundInput::Computer, 0.5, true));
        assert_eq!((s.shared_sounds[1].input.clone(), s.shared_sounds[1].volume), (SoundInput::Microphone { id: "usb-mic".into() }, 1.5));
        s.shared_sounds.clear();
        s.ready_sounds(AudioSource::Both, "x");
        assert!(s.shared_sounds.is_empty(), "only once");
        let mut s = old.clone();
        s.ready_sounds(AudioSource::None, "");
        assert!(s.shared_sounds.is_empty() && s.sounds_ready, "no sound chosen: none shared");
        // New settings start with the computer sound shared, already set up.
        let d = StudioSettings::default();
        assert!(d.sounds_ready && d.shared_sounds[0].input == SoundInput::Computer);
    }

    #[test]
    fn a_scene_has_at_most_six_sounds_and_none_twice() {
        use crate::audio::SoundInput;
        let file = |n: u32| SceneSound::new(SoundInput::File { path: format!("/s/{n}.wav") }, "f");
        let mut s = StudioSettings::default(); // the computer sound shared
        s.scenes[0].sounds = (0..9).map(file).collect();
        s.scenes[0].sounds.insert(0, SceneSound { volume: 9.0, ..SceneSound::new(SoundInput::Computer, "again") });
        s.scenes[0].sounds.push(file(1));
        s.tidy_sounds();
        assert_eq!(s.scene_sounds(0).len(), MAX_SOUNDS, "shared included");
        assert!(!s.scenes[0].sounds.iter().any(|x| x.input == SoundInput::Computer), "a scene's copy of a shared input goes");
        assert!(!s.room_for_sound(0));
        s.scenes[0].sounds.pop();
        assert!(s.room_for_sound(0));
        s.shared_sounds[0].volume = 7.0;
        s.tidy_sounds();
        assert_eq!(s.shared_sounds[0].volume, 2.0, "volumes stay 0..2");
    }

    #[test]
    fn scenes_are_added_removed_and_picked() {
        let mut s = StudioSettings::default();
        assert_eq!(s.add_scene(None), 1);
        assert_eq!(s.scenes[1].name, "Scene 2");
        assert!(s.scenes[1].items.is_empty());
        assert_eq!(s.add_scene(Some("  Talk ")), 2);
        assert_eq!(s.scenes[2].name, "Talk");
        assert_eq!(s.add_scene(Some("   ")), 3, "a blank name gets the usual one");
        assert_eq!(s.scenes[3].name, "Scene 4");
        assert!(s.set_current(9).unwrap_err().contains("no scene 9"));
        s.set_current(2).unwrap();
        // Removing one before the current keeps the same scene current.
        assert_eq!(s.remove_scene(0).unwrap().name, "Scene 1");
        assert_eq!(s.scene().unwrap().name, "Talk");
        // Removing the current one moves to the one before.
        s.remove_scene(1).unwrap();
        assert_eq!(s.current, 0);
        assert_eq!(s.scenes.len(), 2);
        // Removing the last one in the list while it is current.
        s.set_current(1).unwrap();
        s.remove_scene(1).unwrap();
        assert_eq!(s.current, 0);
        assert!(s.remove_scene(0).unwrap_err().contains("at least one"));
        assert!(s.remove_scene(5).unwrap_err().contains("no scene 5"));
    }

    #[test]
    fn studio_video_options_follow_the_settings() {
        let mut s = crate::settings::Settings { rec_format: crate::video::VideoFormat::Gif, video_quality: 3, video_hardware: false, ..Default::default() };
        s.studio.width = 800;
        s.studio.height = 600;
        s.studio.fps = 500;
        s.audio_source = crate::audio::AudioSource::Microphone;
        let o = video_opts(&s);
        assert_eq!(o.format, crate::video::VideoFormat::Mp4, "GIF becomes MP4, as in the window");
        assert_eq!(o.rect, (0, 0, 800, 600));
        assert_eq!((o.fps, o.quality, o.hardware), (120, 3, false));
        assert_eq!(o.audio, crate::audio::AudioSource::Microphone);
        assert!(o.path.file_name().unwrap().to_string_lossy().starts_with("Studio_"));
    }

    /// Bug: the recording used its own stop flag, so an AI app couldn't cancel it.
    #[test]
    fn a_headless_recording_stops_when_told() {
        let dir = std::env::temp_dir().join(format!("snagster-studio-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = StudioSettings {
            scenes: vec![Scene { sounds: Vec::new(), name: "c".into(), items: vec![Item { name: "bg".into(), kind: SourceKind::Color { color: [0, 90, 200, 255] }, x: 0.0, y: 0.0, w: 64.0, h: 64.0, visible: true }] }],
            width: 64,
            height: 64,
            fps: 10,
            ..Default::default()
        };
        let opts = crate::video::VideoOpts::screen((0, 0, 64, 64), 10, crate::video::VideoFormat::Mp4, dir.join("s.mp4"));
        let stop = AtomicBool::new(true);
        let t = Instant::now();
        let _ = record_headless(&st, Some(0), 60, opts, &stop);
        assert!(t.elapsed() < Duration::from_secs(30), "stopped at once, not after 60 s: {:?}", t.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let s = StudioSettings::default();
        let back: StudioSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
        assert!(s.scene().is_some());
    }

    /// A program's sound is kept by its key, so a scene finds it again after a restart.
    #[test]
    fn a_program_sound_is_remembered_by_its_name() {
        use crate::audio::SoundInput;
        let mut s = StudioSettings::default();
        s.scenes[0].sounds.push(SceneSound { volume: 0.7, muted: true, ..SceneSound::new(SoundInput::Program { name: "spotify".into() }, "Spotify") });
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains(r#"{"Program":{"name":"spotify"}}"#), "{j}");
        let back: StudioSettings = serde_json::from_str(&j).unwrap();
        assert_eq!(back.scenes[0].sounds[0], s.scenes[0].sounds[0]);
        assert!(back.sound_inputs().contains(&SoundInput::Program { name: "spotify".into() }));
        assert!(back.mix().contains(&(SoundInput::Program { name: "spotify".into() }, 0.7, true)));
    }

    fn colour(name: &str, c: [u8; 4], x: f32, y: f32, w: f32, h: f32) -> Item {
        Item { name: name.into(), kind: SourceKind::Color { color: c }, x, y, w, h, visible: true }
    }

    fn all_kinds() -> Vec<SourceKind> {
        vec![
            SourceKind::Display { index: 2 },
            SourceKind::Window { title: "Notepad".into() },
            SourceKind::Area { x: -10, y: 20, w: 300, h: 200 },
            SourceKind::Image { path: "C:/pics/logo.png".into() },
            SourceKind::Text { text: "Live \"now\"".into(), size: 36.5, color: [1, 2, 3, 4] },
            SourceKind::Color { color: [9, 8, 7, 6] },
        ]
    }

    #[test]
    fn every_kind_has_its_own_label_and_an_icon() {
        let kinds = all_kinds();
        let mut labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), kinds.len());
        let mut icons: Vec<&str> = kinds.iter().map(|k| k.icon()).collect();
        assert!(icons.iter().all(|i| !i.is_empty()));
        icons.sort();
        icons.dedup();
        assert_eq!(icons.len(), kinds.len());
    }

    #[test]
    fn every_source_kind_round_trips_through_json() {
        let scene = Scene {
            sounds: Vec::new(),
            name: "All".into(),
            items: all_kinds()
                .into_iter()
                .enumerate()
                .map(|(i, kind)| Item { name: format!("s{i}"), kind, x: i as f32 * 1.5, y: -3.25, w: 640.0, h: 360.0, visible: i % 2 == 0 })
                .collect(),
        };
        let st = StudioSettings { scenes: vec![scene.clone(), StudioSettings::default().scenes[0].clone()], current: 1, fps: 60, mic_muted: true, ..Default::default() };
        let back: StudioSettings = serde_json::from_str(&serde_json::to_string_pretty(&st).unwrap()).unwrap();
        assert_eq!(back, st);
        assert_eq!(back.scene(), Some(&st.scenes[1]));
    }

    #[test]
    fn missing_settings_take_their_defaults() {
        // Older settings get their sound set up when loaded (`ready_sounds`, with the old
        // default "computer sound"); then they are the defaults.
        let mut s: StudioSettings = serde_json::from_str("{}").unwrap();
        s.ready_sounds(crate::audio::AudioSource::System, "");
        assert_eq!(s, StudioSettings::default());
        let s: StudioSettings = serde_json::from_str(r#"{"fps": 60, "width": 1280}"#).unwrap();
        assert_eq!((s.fps, s.width, s.height), (60, 1280, 1080));
        assert_eq!(s.scenes.len(), 1);
        assert!((s.system_volume - 1.0).abs() < f32::EPSILON && !s.mic_muted);
        // Unknown fields from a newer Snagster are ignored.
        let s: StudioSettings = serde_json::from_str(r#"{"fps": 24, "future_thing": true}"#).unwrap();
        assert_eq!(s.fps, 24);
    }

    #[test]
    fn the_default_setup_shows_the_first_screen_on_a_full_hd_canvas() {
        let s = StudioSettings::default();
        assert_eq!((s.width, s.height, s.fps, s.current), (1920, 1080, 30, 0));
        let item = &s.scene().unwrap().items[0];
        assert_eq!(item.kind, SourceKind::Display { index: 0 });
        assert_eq!((item.x, item.y, item.w, item.h), (0.0, 0.0, 1920.0, 1080.0));
        assert!(item.visible);
    }

    #[test]
    fn the_current_scene_falls_back_to_the_first() {
        let mut s = StudioSettings::default();
        s.scenes.push(Scene { sounds: Vec::new(), name: "Two".into(), items: Vec::new() });
        s.current = 1;
        assert_eq!(s.scene().unwrap().name, "Two");
        s.current = 99;
        assert_eq!(s.scene().unwrap().name, "Scene 1");
        s.scenes.clear();
        assert!(s.scene().is_none());
    }

    #[test]
    fn sources_can_be_added_removed_and_reordered() {
        let mut scene = Scene { sounds: Vec::new(), name: "s".into(), items: Vec::new() };
        scene.items.push(colour("red", [255, 0, 0, 255], 0.0, 0.0, 8.0, 8.0));
        scene.items.push(colour("blue", [0, 0, 255, 255], 0.0, 0.0, 8.0, 8.0));
        let mut out = RgbaImage::new(8, 8);
        let mut comp = Compositor::default();
        comp.draw(&scene, &mut out);
        assert_eq!(out.get_pixel(4, 4).0, [0, 0, 255, 255], "the later source is on top");
        scene.items.swap(0, 1);
        comp.draw(&scene, &mut out);
        assert_eq!(out.get_pixel(4, 4).0, [255, 0, 0, 255], "moved to the top");
        scene.items.remove(1);
        comp.draw(&scene, &mut out);
        assert_eq!(out.get_pixel(4, 4).0, [0, 0, 255, 255]);
        scene.items.clear();
        comp.draw(&scene, &mut out);
        assert_eq!(out.get_pixel(4, 4).0, [0, 0, 0, 255], "an empty scene is opaque black");
    }

    #[test]
    fn sources_are_placed_at_rounded_canvas_positions() {
        let scene = Scene { sounds: Vec::new(), name: "s".into(), items: vec![colour("c", [0, 255, 0, 255], 2.4, 3.6, 3.5, 1.2)] };
        let mut out = RgbaImage::new(10, 10);
        Compositor::default().draw(&scene, &mut out);
        // x 2.4 -> 2, width 3.5 -> 4 (to x 5); y 3.6 -> 4, height 1.2 -> 1.
        let green: Vec<(u32, u32)> = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).filter(|&(x, y)| out.get_pixel(x, y).0 == [0, 255, 0, 255]).collect();
        assert_eq!(green, [(2, 4), (3, 4), (4, 4), (5, 4)]);
        // A size of zero still draws one pixel; off-canvas sources are clipped, not a panic.
        let scene = Scene { sounds: Vec::new(), name: "s".into(), items: vec![colour("dot", [9, 9, 9, 255], 0.0, 0.0, 0.0, 0.0), colour("off", [1, 1, 1, 255], -50.0, 500.0, 10.0, 10.0)] };
        Compositor::default().draw(&scene, &mut out);
        assert_eq!(out.get_pixel(0, 0).0, [9, 9, 9, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [0, 0, 0, 255]);
    }

    #[test]
    fn see_through_colours_blend_over_what_is_below() {
        let mut out = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        fill(&mut out, 0, 0, 4, 4, [255, 100, 0, 128]);
        let p = out.get_pixel(1, 1).0;
        assert_eq!(p, [128, 50, 0, 255], "half way, alpha unchanged");
        fill(&mut out, 0, 0, 4, 4, [10, 20, 30, 0]);
        assert_eq!(out.get_pixel(1, 1).0, [128, 50, 0, 255], "fully clear changes nothing");
        fill(&mut out, -2, -2, 3, 3, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(1, 1).0, [128, 50, 0, 255], "clipped at the top-left");
        fill(&mut out, 10, 10, 5, 5, [1, 1, 1, 255]);
    }

    #[test]
    fn blending_blits_honour_alpha() {
        let src = RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 50, 0]));
        let mut out = RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        blit_scaled(&mut out, &src, 0, 0, 2, 2, true);
        assert_eq!(out.get_pixel(0, 0).0, [10, 20, 30, 255], "clear pixels leave the canvas alone");
        let src = RgbaImage::from_pixel(2, 2, image::Rgba([210, 120, 30, 255]));
        blit_scaled(&mut out, &src, 0, 0, 2, 2, true);
        assert_eq!(out.get_pixel(1, 1).0, [210, 120, 30, 255], "opaque pixels replace");
        // Without blending, alpha is ignored and the canvas pixel becomes opaque.
        let src = RgbaImage::from_pixel(1, 1, image::Rgba([7, 8, 9, 0]));
        blit_scaled(&mut out, &src, 0, 0, 1, 1, false);
        assert_eq!(out.get_pixel(0, 0).0, [7, 8, 9, 255]);
    }

    #[test]
    fn downscaling_averages_neighbours() {
        let mut src = RgbaImage::new(2, 2);
        src.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
        src.put_pixel(1, 0, image::Rgba([100, 40, 8, 255]));
        src.put_pixel(0, 1, image::Rgba([200, 80, 16, 255]));
        src.put_pixel(1, 1, image::Rgba([100, 40, 0, 255]));
        let mut out = RgbaImage::new(1, 1);
        blit_scaled(&mut out, &src, 0, 0, 1, 1, false);
        assert_eq!(out.get_pixel(0, 0).0, [100, 40, 6, 255]);
        // Upscaling a flat colour keeps it flat.
        let flat = RgbaImage::from_pixel(3, 2, image::Rgba([33, 66, 99, 255]));
        let mut big = RgbaImage::new(17, 11);
        blit_scaled(&mut big, &flat, 0, 0, 17, 11, false);
        assert!(big.pixels().all(|p| p.0 == [33, 66, 99, 255]));
    }

    #[test]
    fn blits_with_nothing_to_draw_do_nothing() {
        let before = RgbaImage::from_pixel(6, 6, image::Rgba([5, 5, 5, 255]));
        let src = RgbaImage::from_pixel(3, 3, image::Rgba([250, 0, 0, 255]));
        for (s, x, y, w, h) in [(&src, 0, 0, 0, 3), (&src, 0, 0, 3, 0), (&src, 6, 0, 3, 3), (&src, 0, -3, 3, 3), (&src, -100, -100, 50, 50)] {
            let mut out = before.clone();
            blit_scaled(&mut out, s, x, y, w, h, false);
            assert_eq!(out, before, "({x}, {y}) {w} x {h}");
        }
        let mut out = before.clone();
        blit_scaled(&mut out, &RgbaImage::new(0, 0), 0, 0, 3, 3, false);
        assert_eq!(out, before);
    }

    #[test]
    fn pictures_draw_once_loaded_and_missing_ones_draw_nothing() {
        let dir = std::env::temp_dir().join(format!("snagster-studio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        RgbaImage::from_pixel(4, 4, image::Rgba([0, 200, 100, 255])).save(&path).unwrap();
        let item = |p: &std::path::Path| Item { name: "p".into(), kind: SourceKind::Image { path: p.to_string_lossy().into() }, x: 0.0, y: 0.0, w: 8.0, h: 8.0, visible: true };
        let mut comp = Compositor::default();
        let mut out = RgbaImage::new(8, 8);
        comp.draw(&Scene { sounds: Vec::new(), name: "s".into(), items: vec![item(&path)] }, &mut out);
        assert_eq!(out.get_pixel(7, 7).0, [0, 200, 100, 255]);
        // The picture is kept: it still draws after the file is gone.
        std::fs::remove_file(&path).unwrap();
        comp.draw(&Scene { sounds: Vec::new(), name: "s".into(), items: vec![item(&path)] }, &mut out);
        assert_eq!(out.get_pixel(3, 3).0, [0, 200, 100, 255]);
        let mut fresh = Compositor::default();
        fresh.draw(&Scene { sounds: Vec::new(), name: "s".into(), items: vec![item(&dir.join("missing.png"))] }, &mut out);
        assert_eq!(out.get_pixel(3, 3).0, [0, 0, 0, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_sources_draw_inside_their_box() {
        let item = Item { name: "t".into(), kind: SourceKind::Text { text: "Hello".into(), size: 40.0, color: [255, 255, 255, 255] }, x: 10.0, y: 10.0, w: 30.0, h: 12.0, visible: true };
        let mut out = RgbaImage::new(64, 64);
        let mut comp = Compositor::default();
        comp.draw(&Scene { sounds: Vec::new(), name: "s".into(), items: vec![item.clone()] }, &mut out);
        let lit = |x: u32, y: u32| out.get_pixel(x, y).0[..3] != [0, 0, 0];
        assert!((10..40).any(|x| (10..22).any(|y| lit(x, y))), "some text is drawn");
        assert!((0..64).all(|x| (0..64).all(|y| !lit(x, y) || ((10..40).contains(&x) && (10..22).contains(&y)))), "nothing outside its box");
        // Drawn again from the cache, the same.
        let first = out.clone();
        comp.draw(&Scene { sounds: Vec::new(), name: "s".into(), items: vec![item] }, &mut out);
        assert_eq!(out, first);
        assert_eq!(comp.texts.len(), 1);
    }

    #[test]
    fn an_empty_area_grabs_nothing() {
        // A zero-sized screen area is skipped before any screen is touched.
        let item = Item { name: "a".into(), kind: SourceKind::Area { x: 0, y: 0, w: 0, h: 10 }, x: 0.0, y: 0.0, w: 8.0, h: 8.0, visible: true };
        let mut out = RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 4]));
        Compositor::default().draw(&Scene { sounds: Vec::new(), name: "s".into(), items: vec![item] }, &mut out);
        assert!(out.pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn the_engine_feeds_frames_as_bgra_until_stopped() {
        let scene = Scene { sounds: Vec::new(), name: "s".into(), items: vec![colour("c", [250, 120, 10, 255], 0.0, 0.0, 16.0, 16.0)] };
        let engine = Engine::start(scene, 4, 4, None);
        let t = Instant::now();
        while engine.latest().is_none() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let frame = engine.latest().expect("a frame");
        assert_eq!(frame.dimensions(), (16, 16), "the canvas is at least 16 x 16");
        assert_eq!(frame.get_pixel(0, 0).0, [250, 120, 10, 255]);
        let feed = engine.feed();
        let mut buf = vec![0u8; 16 * 16 * 4];
        assert!(feed(&mut buf));
        assert_eq!(&buf[..4], &[10, 120, 250, 255], "blue, green, red, opaque");
        assert!(buf.chunks_exact(4).all(|p| p == [10, 120, 250, 255]));
        // A smaller buffer is filled as far as it goes.
        let mut small = vec![0u8; 8];
        assert!(feed(&mut small));
        assert_eq!(small, [10, 120, 250, 255, 10, 120, 250, 255]);
        engine.shared.stop.store(true, Ordering::Relaxed);
        assert!(!feed(&mut buf), "stopped");
    }

    fn ink_line(space: crate::ink::Space, y: f32) -> crate::ink::Stroke {
        crate::ink::Stroke { space, points: vec![[0.0, y], [63.0, y]], color: [255, 255, 0, 255], width: 4.0 }
    }

    /// Ink drawn on the preview is on top of every source; ink drawn on the screen goes on the
    /// source that shows that part of the desktop, under the sources above it.
    #[test]
    fn live_ink_is_composed_into_the_frame() {
        use crate::ink::{Snapshot, Space};
        let screen = Item { name: "area".into(), kind: SourceKind::Area { x: 0, y: 0, w: 64, h: 64 }, x: 0.0, y: 0.0, w: 32.0, h: 32.0, visible: true };
        let scene = Scene { sounds: Vec::new(), name: "s".into(), items: vec![colour("bg", [0, 0, 80, 255], 0.0, 0.0, 64.0, 64.0), screen, colour("cam", [0, 200, 0, 255], 0.0, 0.0, 8.0, 8.0)] };
        let ink = Snapshot { strokes: vec![ink_line(Space::Canvas, 50.0), ink_line(Space::Desktop, 20.0), ink_line(Space::Desktop, 4.0)], alpha: 1.0 };
        let mut out = RgbaImage::new(64, 64);
        let mut comp = Compositor::default();
        comp.draw_with_ink(&scene, &mut out, &ink);
        assert_eq!(out.get_pixel(30, 50).0, [255, 255, 0, 255], "preview ink over the background");
        // Desktop y 20 is canvas y 10 in the half-size area source, only inside its box.
        assert_eq!(out.get_pixel(20, 10).0, [255, 255, 0, 255], "screen ink on the screen source");
        assert_ne!(out.get_pixel(40, 10).0, [255, 255, 0, 255], "not past the source's box");
        assert_eq!(out.get_pixel(4, 2).0, [0, 200, 0, 255], "a source above covers the ink");
        assert_eq!(comp.placed, vec![crate::ink::Placed { desk: (0, 0, 64, 64), canvas: (0, 0, 32, 32) }]);
        // Without ink, the same scene shows none.
        comp.draw(&scene, &mut out);
        assert_eq!(out.get_pixel(30, 50).0, [0, 0, 80, 255]);
    }

    #[test]
    fn the_engine_draws_the_programs_ink() {
        let scene = Scene { sounds: Vec::new(), name: "s".into(), items: vec![colour("c", [0, 0, 0, 255], 0.0, 0.0, 64.0, 64.0)] };
        let _one = crate::ink::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let engine = Engine::start(scene, 64, 64, None);
        engine.shared.ink.store(true, Ordering::Relaxed);
        // A colour no other test uses, at a spot of its own: tests share the program's ink.
        let stroke = crate::ink::Stroke { space: crate::ink::Space::Canvas, points: vec![[40.0, 40.0]], color: [7, 77, 177, 255], width: 10.0 };
        crate::ink::with(|i| {
            i.pen.color = stroke.color;
            i.pen.width = stroke.width;
            i.pen.mode = crate::ink::InkMode::Stay;
            i.begin(stroke.space, stroke.points[0], Instant::now());
        });
        let t = Instant::now();
        while !engine.latest().is_some_and(|f| f.get_pixel(40, 40).0 == [7, 77, 177, 255]) {
            assert!(t.elapsed() < Duration::from_secs(10), "the ink never reached the frame");
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::ink::with(|i| i.clear());
    }

    #[test]
    fn ink_settings_have_defaults_and_give_the_pen() {
        let s: StudioSettings = serde_json::from_str("{}").unwrap();
        assert_eq!((s.ink_mode, s.ink_fade_s, s.ink_color, s.ink_width), (crate::ink::InkMode::Fade, 3.0, [230, 40, 40], 6.0));
        let s = StudioSettings { ink_width: 500.0, ink_fade_s: 0.0, ink_mode: crate::ink::InkMode::Stay, ..Default::default() };
        let p = s.ink_pen();
        assert_eq!((p.width, p.hold_s, p.mode, p.color), (64.0, 0.5, crate::ink::InkMode::Stay, crate::ink::COLORS[0]));
    }

    #[test]
    fn the_engine_follows_scene_changes() {
        let engine = Engine::start(Scene { sounds: Vec::new(), name: "s".into(), items: vec![colour("c", [255, 0, 0, 255], 0.0, 0.0, 16.0, 16.0)] }, 16, 16, None);
        *engine.shared.scene.lock().unwrap() = Scene { sounds: Vec::new(), name: "s".into(), items: vec![colour("c", [0, 255, 0, 255], 0.0, 0.0, 16.0, 16.0)] };
        let t = Instant::now();
        let n = engine.shared.frame_no.load(Ordering::Relaxed);
        loop {
            let green = engine.latest().is_some_and(|f| f.get_pixel(5, 5).0 == [0, 255, 0, 255]);
            if green && engine.shared.frame_no.load(Ordering::Relaxed) > n {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "the new scene was never drawn");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(engine);
    }
}
