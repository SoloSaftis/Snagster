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
    pub system_volume: f32,
    pub system_muted: bool,
    pub mic_volume: f32,
    pub mic_muted: bool,
}

impl Default for StudioSettings {
    fn default() -> Self {
        StudioSettings {
            scenes: vec![Scene {
                name: "Scene 1".into(),
                items: vec![Item { name: "Screen 1".into(), kind: SourceKind::Display { index: 0 }, x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, visible: true }],
            }],
            current: 0,
            width: 1920,
            height: 1080,
            fps: 30,
            system_volume: 1.0,
            system_muted: false,
            mic_volume: 1.0,
            mic_muted: false,
        }
    }
}

impl StudioSettings {
    pub fn scene(&self) -> Option<&Scene> {
        self.scenes.get(self.current).or(self.scenes.first())
    }
}

/// Draws scenes into frames. Keeps pictures and window positions between frames.
#[derive(Default)]
pub struct Compositor {
    pictures: std::collections::HashMap<String, Option<RgbaImage>>,
    windows: std::collections::HashMap<String, (Instant, Option<(i32, i32, u32, u32)>)>,
    screens: Option<(Instant, Vec<crate::capture::MonitorInfo>)>,
    texts: std::collections::HashMap<String, RgbaImage>,
}

impl Compositor {
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
    pub fn draw(&mut self, scene: &Scene, out: &mut RgbaImage) {
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
                    if let Some((x, y, w, h)) = rect
                        && w > 0
                        && h > 0
                        && let Ok(mut img) = crate::capture::grab_rect(x, y, w, h)
                    {
                        if let Some(c) = crate::capture::cursor_pos() {
                            crate::capture::stamp_pointer(&mut img, ((c.0 - x) as f32, (c.1 - y) as f32));
                        }
                        blit_scaled(out, &img, dx, dy, dw, dh, false);
                    }
                }
            }
        }
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
                        comp.draw(&scene, &mut canvas);
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

/// Record a scene for `seconds` without a window (for AI apps and the command line).
pub fn record_headless(settings: &StudioSettings, scene: Option<usize>, seconds: u32, mut opts: crate::video::VideoOpts) -> crate::video::VideoDone {
    let mut s = settings.clone();
    if let Some(i) = scene {
        s.current = i;
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
    let stop = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    crate::video::record(&opts, &stop, &pause)
}

#[cfg(test)]
mod tests {
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
    fn settings_round_trip_through_json() {
        let s = StudioSettings::default();
        let back: StudioSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
        assert!(s.scene().is_some());
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
        let s: StudioSettings = serde_json::from_str("{}").unwrap();
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
        s.scenes.push(Scene { name: "Two".into(), items: Vec::new() });
        s.current = 1;
        assert_eq!(s.scene().unwrap().name, "Two");
        s.current = 99;
        assert_eq!(s.scene().unwrap().name, "Scene 1");
        s.scenes.clear();
        assert!(s.scene().is_none());
    }

    #[test]
    fn sources_can_be_added_removed_and_reordered() {
        let mut scene = Scene { name: "s".into(), items: Vec::new() };
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
        let scene = Scene { name: "s".into(), items: vec![colour("c", [0, 255, 0, 255], 2.4, 3.6, 3.5, 1.2)] };
        let mut out = RgbaImage::new(10, 10);
        Compositor::default().draw(&scene, &mut out);
        // x 2.4 -> 2, width 3.5 -> 4 (to x 5); y 3.6 -> 4, height 1.2 -> 1.
        let green: Vec<(u32, u32)> = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).filter(|&(x, y)| out.get_pixel(x, y).0 == [0, 255, 0, 255]).collect();
        assert_eq!(green, [(2, 4), (3, 4), (4, 4), (5, 4)]);
        // A size of zero still draws one pixel; off-canvas sources are clipped, not a panic.
        let scene = Scene { name: "s".into(), items: vec![colour("dot", [9, 9, 9, 255], 0.0, 0.0, 0.0, 0.0), colour("off", [1, 1, 1, 255], -50.0, 500.0, 10.0, 10.0)] };
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
        comp.draw(&Scene { name: "s".into(), items: vec![item(&path)] }, &mut out);
        assert_eq!(out.get_pixel(7, 7).0, [0, 200, 100, 255]);
        // The picture is kept: it still draws after the file is gone.
        std::fs::remove_file(&path).unwrap();
        comp.draw(&Scene { name: "s".into(), items: vec![item(&path)] }, &mut out);
        assert_eq!(out.get_pixel(3, 3).0, [0, 200, 100, 255]);
        let mut fresh = Compositor::default();
        fresh.draw(&Scene { name: "s".into(), items: vec![item(&dir.join("missing.png"))] }, &mut out);
        assert_eq!(out.get_pixel(3, 3).0, [0, 0, 0, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_sources_draw_inside_their_box() {
        let item = Item { name: "t".into(), kind: SourceKind::Text { text: "Hello".into(), size: 40.0, color: [255, 255, 255, 255] }, x: 10.0, y: 10.0, w: 30.0, h: 12.0, visible: true };
        let mut out = RgbaImage::new(64, 64);
        let mut comp = Compositor::default();
        comp.draw(&Scene { name: "s".into(), items: vec![item.clone()] }, &mut out);
        let lit = |x: u32, y: u32| out.get_pixel(x, y).0[..3] != [0, 0, 0];
        assert!((10..40).any(|x| (10..22).any(|y| lit(x, y))), "some text is drawn");
        assert!((0..64).all(|x| (0..64).all(|y| !lit(x, y) || ((10..40).contains(&x) && (10..22).contains(&y)))), "nothing outside its box");
        // Drawn again from the cache, the same.
        let first = out.clone();
        comp.draw(&Scene { name: "s".into(), items: vec![item] }, &mut out);
        assert_eq!(out, first);
        assert_eq!(comp.texts.len(), 1);
    }

    #[test]
    fn an_empty_area_grabs_nothing() {
        // A zero-sized screen area is skipped before any screen is touched.
        let item = Item { name: "a".into(), kind: SourceKind::Area { x: 0, y: 0, w: 0, h: 10 }, x: 0.0, y: 0.0, w: 8.0, h: 8.0, visible: true };
        let mut out = RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 4]));
        Compositor::default().draw(&Scene { name: "s".into(), items: vec![item] }, &mut out);
        assert!(out.pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }

    #[test]
    fn the_engine_feeds_frames_as_bgra_until_stopped() {
        let scene = Scene { name: "s".into(), items: vec![colour("c", [250, 120, 10, 255], 0.0, 0.0, 16.0, 16.0)] };
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

    #[test]
    fn the_engine_follows_scene_changes() {
        let engine = Engine::start(Scene { name: "s".into(), items: vec![colour("c", [255, 0, 0, 255], 0.0, 0.0, 16.0, 16.0)] }, 16, 16, None);
        *engine.shared.scene.lock().unwrap() = Scene { name: "s".into(), items: vec![colour("c", [0, 255, 0, 255], 0.0, 0.0, 16.0, 16.0)] };
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
