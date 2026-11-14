//! What goes out live: the whole screen, a window, a desktop area or Studio's scene.
//!
//! Screen, window and area are copied by a capture thread of their own at the stream's frame
//! rate; the stream's picture thread only takes the newest copy, so a slow copy repeats a
//! frame instead of holding up the encoder or the network. Studio's scene comes from Studio's
//! compositor, which already runs on its own thread.

use super::engine::FrameSource;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum Source {
    /// The screen the mouse is on when going live.
    #[default]
    Screen,
    /// A window, found by part of its title (it may move; it is followed).
    Window(String),
    /// A desktop rectangle: left, top, width, height.
    Area(i32, i32, u32, u32),
    /// Studio's current scene, composed with all its sources.
    Studio,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::Screen => "Whole screen",
            Source::Window(_) => "A window",
            Source::Area(..) => "An area",
            Source::Studio => "Studio scene",
        }
    }

    /// From the MCP / command-line name.
    pub fn parse(s: &str) -> Option<Source> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "screen" | "whole screen" | "display" | "monitor" => Source::Screen,
            "window" => Source::Window(String::new()),
            "area" | "region" => Source::Area(0, 0, 0, 0),
            "studio" | "scene" | "studio scene" => Source::Studio,
            _ => return None,
        })
    }
}

/// The size a picture of `w` × `h` gets in a frame of `fw` × `fh`, kept in proportion, and where
/// it goes (centred).
pub fn fit(w: u32, h: u32, fw: u32, fh: u32) -> (i64, i64, u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0, fw, fh);
    }
    let s = (fw as f64 / w as f64).min(fh as f64 / h as f64);
    let (dw, dh) = (((w as f64 * s).round() as u32).clamp(1, fw), ((h as f64 * s).round() as u32).clamp(1, fh));
    (((fw - dw) / 2) as i64, ((fh - dh) / 2) as i64, dw, dh)
}

/// Draw `src` into `out` in proportion (black around it). Same size: a plain copy.
pub fn draw_fitted(out: &mut RgbaImage, src: &RgbaImage) {
    if out.dimensions() == src.dimensions() {
        out.copy_from_slice(src.as_raw());
        return;
    }
    for p in out.pixels_mut() {
        *p = image::Rgba([0, 0, 0, 255]);
    }
    let (x, y, w, h) = fit(src.width(), src.height(), out.width(), out.height());
    crate::studio::blit_scaled(out, src, x, y, w, h, false);
}

/// The newest picture, shared between a capture thread and the stream.
#[derive(Default)]
struct Latest {
    frame: Mutex<Option<Arc<RgbaImage>>>,
    stop: AtomicBool,
}

/// A capture thread for a screen, window or area. Stops when dropped.
pub struct Grabber {
    latest: Arc<Latest>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Where a window is now (found again every 0.7 s).
fn window_rect(title: &str) -> Option<(i32, i32, u32, u32)> {
    let low = title.to_lowercase();
    crate::capture::list_windows().into_iter().find(|w| w.label.to_lowercase().contains(&low)).map(|w| (w.x, w.y, w.width, w.height))
}

impl Grabber {
    /// Copy `rect` (or the window called `window`, following it) `fps` times a second into
    /// frames of `w` × `h`, with the mouse pointer when `cursor`.
    pub fn start(rect: (i32, i32, u32, u32), window: Option<String>, w: u32, h: u32, fps: u32, cursor: bool) -> Grabber {
        let latest = Arc::new(Latest::default());
        let l = latest.clone();
        let thread = std::thread::Builder::new()
            .name("snagster-stream-capture".into())
            .spawn(move || {
                crate::capture::background_thread(false);
                let every = Duration::from_nanos(1_000_000_000 / fps.clamp(1, 120) as u64);
                let mut rect = rect;
                let mut found = Instant::now();
                let mut next = Instant::now();
                while !l.stop.load(Ordering::Relaxed) {
                    if let Some(t) = &window
                        && found.elapsed() > Duration::from_millis(700)
                    {
                        found = Instant::now();
                        if let Some(r) = window_rect(t) {
                            rect = r;
                        }
                    }
                    if rect.2 > 0
                        && rect.3 > 0
                        && let Ok(mut img) = crate::capture::grab_rect(rect.0, rect.1, rect.2, rect.3)
                    {
                        if cursor && let Some(c) = crate::capture::cursor_pos() {
                            crate::capture::stamp_pointer(&mut img, ((c.0 - rect.0) as f32, (c.1 - rect.1) as f32));
                        }
                        let mut out = RgbaImage::new(w, h);
                        draw_fitted(&mut out, &img);
                        if let Ok(mut f) = l.frame.lock() {
                            *f = Some(Arc::new(out));
                        }
                    }
                    next += every;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now;
                    }
                }
            })
            .ok();
        Grabber { latest, thread }
    }

    /// The stream's picture source: the newest copy (black until the first one).
    pub fn frames(&self) -> FrameSource {
        let l = self.latest.clone();
        FrameSource::Rgba(Arc::new(move |buf: &mut [u8]| {
            if l.stop.load(Ordering::Relaxed) {
                return false;
            }
            if let Some(f) = l.frame.lock().ok().and_then(|f| f.clone()) {
                let n = buf.len().min(f.as_raw().len());
                buf[..n].copy_from_slice(&f.as_raw()[..n]);
            }
            true
        }))
    }

    /// A picture has been copied (for waiting at the start).
    pub fn ready(&self) -> bool {
        self.latest.frame.lock().map(|f| f.is_some()).unwrap_or(false)
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        self.latest.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Studio's composed scene, brought to `w` × `h` when the stream's size differs from the
/// canvas (a service that takes less than the canvas size).
pub fn studio_frames(sh: Arc<crate::studio::Shared>, w: u32, h: u32) -> FrameSource {
    let scaled = Mutex::new((0u64, RgbaImage::new(w, h)));
    FrameSource::Rgba(Arc::new(move |buf: &mut [u8]| {
        if sh.stop.load(Ordering::Relaxed) {
            return false;
        }
        let Some(f) = sh.frame.lock().ok().and_then(|f| f.clone()) else { return true };
        if f.dimensions() == (w, h) {
            let n = buf.len().min(f.as_raw().len());
            buf[..n].copy_from_slice(&f.as_raw()[..n]);
            return true;
        }
        // Scale each new frame once.
        let no = sh.frame_no.load(Ordering::Relaxed);
        if let Ok(mut s) = scaled.lock() {
            if s.0 != no {
                draw_fitted(&mut s.1, &f);
                s.0 = no;
            }
            let n = buf.len().min(s.1.as_raw().len());
            buf[..n].copy_from_slice(&s.1.as_raw()[..n]);
        }
        true
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_in_proportion_and_centred() {
        assert_eq!(fit(1920, 1080, 1280, 720), (0, 0, 1280, 720));
        // A tall window in a wide frame: bars left and right.
        assert_eq!(fit(500, 1000, 1280, 720), (460, 0, 360, 720));
        assert_eq!(fit(4000, 1000, 1000, 1000), (0, 375, 1000, 250));
        assert_eq!(fit(0, 10, 64, 64), (0, 0, 64, 64));
    }

    #[test]
    fn drawing_fitted_copies_or_scales_with_black_bars() {
        let red = RgbaImage::from_pixel(10, 20, image::Rgba([255, 0, 0, 255]));
        let mut same = RgbaImage::new(10, 20);
        draw_fitted(&mut same, &red);
        assert_eq!(same, red);
        let mut wide = RgbaImage::from_pixel(40, 20, image::Rgba([9, 9, 9, 9]));
        draw_fitted(&mut wide, &red);
        assert_eq!(wide.get_pixel(0, 10).0, [0, 0, 0, 255], "a bar");
        assert_eq!(wide.get_pixel(20, 10).0, [255, 0, 0, 255], "the picture in the middle");
    }

    #[test]
    fn sources_have_names_and_parse() {
        assert_eq!(Source::parse("Screen"), Some(Source::Screen));
        assert_eq!(Source::parse("window"), Some(Source::Window(String::new())));
        assert_eq!(Source::parse("area"), Some(Source::Area(0, 0, 0, 0)));
        assert_eq!(Source::parse("studio"), Some(Source::Studio));
        assert_eq!(Source::parse("webcam"), None);
        for s in [Source::Screen, Source::Window("x".into()), Source::Area(1, 2, 3, 4), Source::Studio] {
            assert!(!s.label().is_empty());
            let j = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<Source>(&j).unwrap(), s);
        }
    }

    #[test]
    fn studio_frames_are_scaled_to_the_stream() {
        let scene = crate::studio::Scene {
            name: "t".into(),
            items: vec![crate::studio::Item { name: "c".into(), kind: crate::studio::SourceKind::Color { color: [0, 200, 0, 255] }, x: 0.0, y: 0.0, w: 64.0, h: 64.0, visible: true }],
        };
        let e = crate::studio::Engine::start(scene, 64, 64, None);
        let t = Instant::now();
        while e.latest().is_none() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let FrameSource::Rgba(f) = studio_frames(e.shared.clone(), 32, 16) else { panic!("an RGBA source") };
        let mut buf = vec![0u8; 32 * 16 * 4];
        assert!(f(&mut buf));
        // 64 x 64 into 32 x 16: a 16 x 16 square in the middle, black either side.
        let px = |x: usize, y: usize| &buf[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(px(16, 8), &[0, 200, 0, 255]);
        assert_eq!(px(1, 8), &[0, 0, 0, 255]);
        drop(e);
    }
}
