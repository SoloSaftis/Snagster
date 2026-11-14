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
#[cfg(test)]
pub fn fit(w: u32, h: u32, fw: u32, fh: u32) -> (i64, i64, u32, u32) {
    let (x, y, dw, dh) = crate::video::fit_rect(w, h, fw, fh);
    (x as i64, y as i64, dw, dh)
}

/// Draw `src` into `out` in proportion (black around it). Same size: a plain copy.
pub fn draw_fitted(out: &mut RgbaImage, src: &RgbaImage) {
    if out.dimensions() == src.dimensions() {
        out.copy_from_slice(src.as_raw());
        return;
    }
    let (w, h) = out.dimensions();
    let mut f = crate::video::Frame { width: w, height: h, rgba: std::mem::take(out).into_raw(), number: 0 };
    let px = crate::video::Pixels { data: src.as_raw(), width: src.width(), height: src.height(), stride: src.width() as usize * 4, bgra: false };
    crate::video::fit_into(&px, &mut f, (w, h));
    *out = RgbaImage::from_raw(w, h, f.rgba).unwrap_or_else(|| RgbaImage::new(w, h));
}

/// How often a followed window is looked for again (it may have moved).
const FOLLOW_EVERY: Duration = Duration::from_millis(300);

/// Where a window is now.
fn window_rect(title: &str) -> Option<(i32, i32, u32, u32)> {
    let low = title.to_lowercase();
    crate::capture::list_windows().into_iter().find(|w| w.label.to_lowercase().contains(&low)).map(|w| (w.x, w.y, w.width, w.height))
}

/// A screen, window or area copied by `video::ScreenFeed` (Desktop Duplication on Windows) at
/// the stream's size, and for a window a thread that follows it when it moves. Stops when
/// dropped.
pub struct Grabber {
    feed: Option<Arc<crate::video::ScreenFeed>>,
    error: Option<String>,
    stop: Arc<AtomicBool>,
    follow: Option<std::thread::JoinHandle<()>>,
}

impl Grabber {
    /// Copy `rect` (or the window called `window`, following it) `fps` times a second into
    /// frames of `w` × `h`, with the mouse pointer when `cursor`. Black when it can't be copied.
    pub fn start(rect: (i32, i32, u32, u32), window: Option<String>, w: u32, h: u32, fps: u32, cursor: bool) -> Grabber {
        Grabber::open(rect, window, crate::video::FeedOptions { fps, cursor, fit: Some((w, h)), every_frame: false })
    }

    /// `start` with all the feed's options (`every_frame` for measuring).
    pub fn open(rect: (i32, i32, u32, u32), window: Option<String>, o: crate::video::FeedOptions) -> Grabber {
        let stop = Arc::new(AtomicBool::new(false));
        let (feed, error) = match crate::video::screen_feed(rect, o) {
            Ok(f) => (Some(Arc::new(f)), None),
            Err(e) => (None, Some(e)),
        };
        let follow = match (&feed, window) {
            (Some(f), Some(title)) => {
                let (f, s) = (f.clone(), stop.clone());
                std::thread::Builder::new()
                    .name("snagster-stream-follow".into())
                    .spawn(move || {
                        let mut last = Instant::now();
                        while !s.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(30));
                            if last.elapsed() >= FOLLOW_EVERY {
                                last = Instant::now();
                                if let Some(r) = window_rect(&title) {
                                    f.move_to(r);
                                }
                            }
                        }
                    })
                    .ok()
            }
            _ => None,
        };
        Grabber { feed, error, stop, follow }
    }

    /// The stream's picture source: the newest copy (black until the first one).
    pub fn frames(&self) -> FrameSource {
        let (feed, stop) = (self.feed.clone(), self.stop.clone());
        FrameSource::Rgba(Arc::new(move |buf: &mut [u8]| {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            if let Some(f) = &feed {
                f.with_latest(|p| {
                    if p.number > 0 && p.rgba.len() == buf.len() {
                        buf.copy_from_slice(&p.rgba);
                    }
                });
            }
            true
        }))
    }

    /// Pictures copied so far.
    pub fn captured(&self) -> u64 {
        self.feed.as_ref().map_or(0, |f| f.frames())
    }

    /// A picture has been copied (for waiting at the start).
    pub fn ready(&self) -> bool {
        self.captured() > 0
    }

    /// How the screen is copied ("graphics chip", "GDI", "screen grab").
    pub fn method(&self) -> &'static str {
        self.feed.as_ref().map_or("", |f| f.method())
    }

    /// Why nothing could be copied, or what went wrong last.
    pub fn error(&self) -> Option<String> {
        self.error.clone().or_else(|| self.feed.as_ref().and_then(|f| f.error()))
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.follow.take() {
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
