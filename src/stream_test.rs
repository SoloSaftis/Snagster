//! `snagster --stream-test [quick]`: stream into Snagster's own test RTMP server on 127.0.0.1
//! (nothing leaves the computer) with the best encoder here, and report per case the new
//! pictures a second, the frame rate that reached the server, processor use and where the time
//! went. Quick (CI): 720p30 for 3 s. Full: 720p30, 1080p30, 1440p60, 2160p60 and 1080p120.
//!
//! The screen is copied where it is at least as tall as the case; for a bigger case (4K on a
//! 1600-line screen) a moving test picture of the case's size goes through the encoder and the
//! network instead, and the screen copy's own speed is measured on its own at the screen's size
//! (the report says which is which). Zero-copy (`SNAGSTER_ZERO_COPY=0` turns it off) is used
//! where it starts, as when going live.

use crate::stream::engine::{Encoders, FrameSource, SoundSource, StreamConfig, Streamer, Timing};
use crate::stream::source::Grabber;
use crate::stream::{Service, State, StreamTarget};
use crate::{capture, encode, stream, video};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// One case: a picture `height` tall at `fps` for `secs` seconds; `every_frame` copies the
/// screen every frame even when it doesn't change (the cost of a screen that always changes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Case {
    pub height: u32,
    pub fps: u32,
    pub secs: u64,
    pub every_frame: bool,
}

const fn case(height: u32, fps: u32, secs: u64, every_frame: bool) -> Case {
    Case { height, fps, secs, every_frame }
}

/// The cases of a run.
pub fn cases(quick: bool) -> Vec<Case> {
    if quick {
        vec![case(720, 30, 3, true)]
    } else {
        vec![case(720, 30, 5, true), case(1080, 30, 6, false), case(1080, 30, 6, true), case(1440, 60, 6, false), case(1440, 60, 6, true), case(2160, 60, 6, true), case(1080, 120, 6, true)]
    }
}

/// What a case streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseSource {
    /// The screen, brought to the case's size.
    Screen,
    /// The screen is smaller than the case: a moving test picture at the case's size.
    TestPicture,
}

/// The screen where it is at least as tall as the case, else the test picture.
pub fn case_source(screen_h: u32, case_h: u32) -> CaseSource {
    if screen_h >= case_h { CaseSource::Screen } else { CaseSource::TestPicture }
}

/// The stream's size: `height` tall in the screen's proportions for the screen, 16:9 for the
/// test picture (even numbers).
pub fn case_size(sw: u32, sh: u32, height: u32, source: CaseSource) -> (u32, u32) {
    match source {
        CaseSource::Screen => stream_size(sw, sh, height),
        CaseSource::TestPicture => ((height * 16 / 9) & !1, height & !1),
    }
}

/// A stream size `height` pixels tall in the screen's proportions (even numbers).
pub fn stream_size(sw: u32, sh: u32, height: u32) -> (u32, u32) {
    let h = height.min(sh.max(2));
    let w = (sw as u64 * h as u64 / sh.max(1) as u64) as u32;
    (w.max(2) & !1, h.max(2) & !1)
}

/// How a case went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    /// The encoder itself is too slow for this case here: reported, not failed.
    CantHere,
}

/// 90 % of the frame rate must reach the server wherever the encoder (measured alone at this
/// size just before, on the path the stream uses) manages at least 1.2 times the rate; where it
/// doesn't, the machine can't, which is reported without failing. `copies_ok`: the screen copy
/// made enough new pictures (or wasn't asked to); `healthy`: no error, not failed.
pub fn verdict(fps: u32, reached: f64, encoder_fps: Option<f64>, copies_ok: bool, healthy: bool) -> Verdict {
    let fps = fps as f64;
    if !healthy {
        return Verdict::Fail;
    }
    match encoder_fps {
        Some(e) if e < 1.2 * fps => Verdict::CantHere,
        _ if reached >= 0.9 * fps && copies_ok => Verdict::Pass,
        _ => Verdict::Fail,
    }
}

/// New pictures a second the screen copy must make when it copies every frame: 80 % of the
/// stream's. Every system's copy manages far more on the CI machines (Windows Desktop
/// Duplication, Linux X11 shared memory 4.5 ms a grab, the Mac display copy about 22 ms), so
/// this catches a copy that has become slow (xcap's X11 path made 3 a second) while leaving
/// room for a virtual machine's hiccups. A screen copied only when it changes: no minimum.
pub fn wanted_copies(fps: u32, every_frame: bool) -> f64 {
    if every_frame { 0.8 * fps as f64 } else { 0.0 }
}

/// `grabs` screen copies that took `us` microseconds in all over `secs` seconds, for a person.
pub fn grab_summary(grabs: u64, us: u64, secs: f64) -> String {
    let each = if grabs == 0 { 0.0 } else { us as f64 / grabs as f64 / 1000.0 };
    let most = if each > 0.0 { 1000.0 / each } else { 0.0 };
    format!("{:.1} grabs a second, {each:.2} ms each (at most {most:.0} a second)", grabs as f64 / secs.max(1e-6))
}

/// The encoder `open_video` would pick here, alone, at `w` × `h` with pictures from memory: its
/// name and frames a second (None when none opens).
fn memory_encoder_speed(w: u32, h: u32, frames: u32) -> Option<(String, f64)> {
    for b in encode::plan(encode::Os::current(), encode::Codec::H264, true) {
        if let Ok((n, f)) = encode::speed(b, w, h, frames) {
            return Some((n.name, f));
        }
    }
    None
}

/// The zero-copy encoder alone: `frames` pictures of the grabber's newest surface, as fast as
/// it goes (frames a second).
#[cfg(zero_copy)]
fn surface_encoder_speed(g: &Grabber, encoders: &Encoders, cfg: &StreamConfig, frames: u32) -> Result<f64, String> {
    let FrameSource::Surfaces(f) = g.frames() else { return Err("not a zero-copy source".into()) };
    let s = f().flatten().ok_or("no picture yet")?;
    let v = stream::engine::video_config(cfg)?;
    let mut e = (encoders.video)(&v)?;
    let t = Instant::now();
    let mut out = 0usize;
    for i in 0..frames {
        out += e.encode_surface(&s, i as i64 * v.frame_us(), i == 0)?.len();
    }
    out += e.finish()?.len();
    if out + 4 < frames as usize {
        return Err(format!("{out} of {frames} frames came out"));
    }
    Ok(frames as f64 / t.elapsed().as_secs_f64().max(1e-6))
}

/// The zero-copy encoder's speed alone, printed (None when it can't be measured).
#[cfg(zero_copy)]
fn zero_copy_speed(g: &Grabber, encoders: &Encoders, cfg: &StreamConfig, frames: u32) -> Option<f64> {
    match surface_encoder_speed(g, encoders, cfg, frames) {
        Ok(f) => {
            println!("     encoder alone, pictures in graphics memory: {f:.0} fps");
            Some(f)
        }
        Err(e) => {
            println!("     zero-copy encoder alone: {e}");
            None
        }
    }
}

#[cfg(not(zero_copy))]
fn zero_copy_speed(_g: &Grabber, _encoders: &Encoders, _cfg: &StreamConfig, _frames: u32) -> Option<f64> {
    None
}

/// A zero-copy test picture at `w` × `h` and encoders on its device (Windows).
#[cfg(windows)]
fn zero_copy_test_picture(cfg: &StreamConfig, encoders: &Encoders) -> Result<(Grabber, Encoders), String> {
    let want = stream::live::zero_copy_format(cfg.hardware, encoders.zero_copy).ok_or("zero-copy is off")?;
    let g = Grabber::test_surfaces(cfg.width, cfg.height, cfg.fps)?;
    stream::live::zero_copy_encoders(g, want, cfg, encoders)
}

#[cfg(not(windows))]
fn zero_copy_test_picture(_cfg: &StreamConfig, _encoders: &Encoders) -> Result<(Grabber, Encoders), String> {
    Err("the test picture goes through memory here".into())
}

/// Run the test; true: all passed.
pub fn run(quick: bool) -> bool {
    let (mx, my, sw, sh) = capture::monitor_under_cursor().unwrap_or((0, 0, 1280, 720));
    println!("Screen {sw}x{sh} at {mx},{my}");
    // A system whose sleeps run long can't keep a frame rate: how long a 5 ms sleep takes on a
    // plain thread, and on a timely one while the stream's activity is held (what streaming uses).
    let plain = video::measured_sleep_ms(5, 20);
    let (timely, paced, mode) = std::thread::spawn(|| {
        let _a = video::TimelyActivity::begin("Stream test");
        video::timely_thread();
        let slept = video::measured_sleep_ms(5, 20);
        let mut p = video::Pacer::calibrated();
        let t = Instant::now();
        for _ in 0..20 {
            p.wait_until(Instant::now() + Duration::from_millis(5));
        }
        (slept, t.elapsed().as_secs_f64() * 1000.0 / 20.0, p.mode())
    })
    .join()
    .unwrap_or((0.0, 0.0, String::new()));
    println!("A 5 ms sleep takes {plain:.1} ms ({timely:.1} ms on a streaming thread); a paced 5 ms wait takes {paced:.1} ms; pacing: {mode}");
    let mut failed = 0;
    let zero_copy = stream::live::zero_copy_format(true, true);
    println!("Zero-copy: {}", match zero_copy {
        Some(f) => format!("{f:?} where it starts (SNAGSTER_ZERO_COPY=0 turns it off)"),
        None if encode::zero_copy_disabled() => "off (SNAGSTER_ZERO_COPY=0)".into(),
        None => "not on this system (pictures go through memory)".into(),
    });
    #[cfg(windows)]
    match video::gpu_conversion_check() {
        Ok((dy, duv, adapter)) => {
            let ok = dy <= 3 && duv <= 4;
            // Only a full run fails on it (CI's Windows has no graphics chip, only Microsoft's
            // software renderer, and no hardware encoder that would use the conversion).
            if !ok && !quick {
                failed += 1;
            }
            println!("{} graphics chip BGRA to NV12 ({adapter}) against Snagster's own conversion: Y differs by at most {dy}, U/V by {duv} (want 3 and 4 at most)", if ok { "ok  " } else { "FAIL" });
        }
        Err(e) => println!("     graphics chip conversion not checked: {e}"),
    }
    // OpenH264's speed shows whether its assembly was built (nasm installed where it was compiled).
    match encode::speed(encode::Backend::OpenH264, 1920, 1080, if quick { 60 } else { 150 }) {
        Ok((_, f)) => println!("OpenH264 1920x1080 encode speed: {f:.0} fps"),
        Err(e) => {
            println!("FAIL OpenH264: {e}");
            failed += 1;
        }
    }
    let encoders = Encoders::system();
    // Without screen access (macOS without the permission or a logged-in desktop) the cases
    // that copy the screen are skipped, not failed; the test-picture cases still run and count.
    let skip = capture::check_skip(capture::screen_access());
    for c in cases(quick) {
        let srv = match stream::testserver::TestServer::start() {
            Ok(s) => s,
            Err(e) => {
                println!("FAIL no local test server: {e}");
                failed += 1;
                continue;
            }
        };
        let source = case_source(sh, c.height);
        if let (Some(why), CaseSource::Screen) = (skip, source) {
            println!("{why}: {}p{}{}", c.height, c.fps, if c.every_frame { ", every frame" } else { "" });
            continue;
        }
        let (w, h) = case_size(sw, sh, c.height, source);
        let target = StreamTarget { service: Service::Custom, url: srv.url(), key: "stream-test".into() };
        let cfg = StreamConfig::for_target(target, w, h, c.fps, encode::Codec::H264);
        let (w, h, fps) = (cfg.width, cfg.height, cfg.fps);
        println!("-- {w}x{h} {fps} fps ({}p{}){}", c.height, c.fps, if c.every_frame { ", a new picture every frame" } else { ", the screen as it is" });
        // The test picture's case: how fast the screen itself copies, at its own size.
        if let Some(why) = skip {
            println!("     screen copy alone at the screen's own size {why}");
        } else if source == CaseSource::TestPicture {
            native_copy(mx, my, sw, sh, fps, zero_copy.is_some());
        }
        // The encoder alone at this size, through memory: a slow encoder told apart from a slow
        // copy or network.
        let memory_speed = memory_encoder_speed(w, h, if w * h > 2560 * 1440 { 60 } else { 90 });
        match &memory_speed {
            Some((n, f)) => println!("     encoder alone, pictures from memory: {n} {f:.0} fps"),
            None => println!("     no encoder opens at {w}x{h}"),
        }
        // The picture source and the encoders for it: zero-copy where it starts.
        let started = match source {
            CaseSource::Screen => {
                let feed = video::FeedOptions { fps, cursor: true, fit: Some((w, h)), every_frame: c.every_frame };
                stream::live::zero_copy(true, (mx, my, sw, sh), None, feed, &cfg, &encoders)
            }
            CaseSource::TestPicture => zero_copy_test_picture(&cfg, &encoders),
        };
        let counted = Arc::new(AtomicU64::new(0));
        let (grabber, encs, zc) = match started {
            Ok((g, e)) => (Some(g), e, true),
            Err(why) => {
                if zero_copy.is_some() {
                    println!("     zero-copy didn't start ({why}): pictures go through memory");
                }
                let g = (source == CaseSource::Screen).then(|| Grabber::open((mx, my, sw, sh), None, video::FeedOptions { fps, cursor: true, fit: Some((w, h)), every_frame: c.every_frame }));
                (g, encoders.clone(), false)
            }
        };
        if let Some(g) = &grabber {
            let t = Instant::now();
            while !g.ready() && t.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let grabber = grabber.filter(|g| g.ready() || source == CaseSource::TestPicture);
        // The encoder's speed on the path this stream uses (the faster of the two measured).
        let surface_fps = if zc { grabber.as_ref().and_then(|g| zero_copy_speed(g, &encs, &cfg, if w * h > 2560 * 1440 { 60 } else { 90 })) } else { None };
        let encoder_fps = match (memory_speed.as_ref().map(|s| s.1), surface_fps) {
            (Some(m), Some(z)) => Some(m.max(z)),
            (m, z) => m.or(z),
        };
        let (frames, what) = match &grabber {
            Some(g) if source == CaseSource::Screen => (g.frames(), format!("screen (copied by {}{})", g.method(), if c.every_frame { ", every frame" } else { ", when it changes" })),
            Some(g) => (g.frames(), format!("moving test picture ({}; the screen is only {sw}x{sh})", g.method())),
            None => {
                let pic = encode::MovingPicture::new(w, h);
                let (n, k) = (AtomicU64::new(0), counted.clone());
                let f: stream::engine::RgbaFeed = Arc::new(move |buf: &mut [u8]| {
                    pic.frame(n.fetch_add(1, Ordering::Relaxed) as u32, buf);
                    k.fetch_add(1, Ordering::Relaxed);
                    true
                });
                let why = if source == CaseSource::TestPicture { format!("the screen is only {sw}x{sh}") } else { "the screen couldn't be copied".into() };
                (FrameSource::Rgba(f), format!("moving test picture from memory ({why})"))
            }
        };
        let s = match Streamer::start(cfg, frames, SoundSource::None, encs) {
            Ok(s) => s,
            Err(e) => {
                println!("FAIL {w}x{h} {fps} fps: {e}");
                failed += 1;
                continue;
            }
        };
        let t = Instant::now();
        while s.stats().state != State::Live && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(500));
        let made = |g: &Option<Grabber>| g.as_ref().map_or(counted.load(Ordering::Relaxed), |g| g.captured());
        let (v0, g0, c0, t0) = (srv.video_messages(), made(&grabber), crate::cpu_seconds(), Instant::now());
        let (timing0, grabs0) = (s.stats().timing, grabber.as_ref().map(|g| g.grab_time()).unwrap_or_default());
        std::thread::sleep(Duration::from_secs(c.secs));
        let (v1, g1, c1, wall) = (srv.video_messages(), made(&grabber), crate::cpu_seconds(), t0.elapsed().as_secs_f64());
        let (timing1, grabs1) = (s.stats().timing, grabber.as_ref().map(|g| g.grab_time()).unwrap_or_default());
        let stats = s.stop();
        drop(grabber);
        let copied = (g1 - g0) as f64 / wall;
        let got = (v1 - v0) as f64 / wall;
        let cpu = 100.0 * (c1 - c0) / wall;
        let want_copies = wanted_copies(fps, c.every_frame && source == CaseSource::Screen);
        let healthy = stats.error.is_none() && stats.state != State::Failed;
        let v = verdict(fps, got, encoder_fps, copied >= want_copies, healthy);
        if v == Verdict::Fail {
            failed += 1;
        }
        let d: Timing = timing1.since(&timing0);
        let encode_ms = if d.frames_in == 0 { 0.0 } else { d.encode_us as f64 / d.frames_in as f64 / 1000.0 };
        println!(
            "{} {w}x{h} {fps} fps of the {what}: {copied:.1} new pictures a second, {got:.1} fps reached the server (want {:.0}+), {} dropped, CPU {cpu:.1}% of one core, encode {encode_ms:.2} ms a picture, encoder {}{}{}",
            match v {
                Verdict::Pass => "ok  ",
                Verdict::Fail => "FAIL",
                Verdict::CantHere => "can't",
            },
            0.9 * fps as f64,
            stats.frames_dropped,
            stats.video_encoder.map(|e| e.name).unwrap_or_default(),
            if zc { ", zero-copy" } else { ", through memory" },
            stats.error.map(|e| format!(", error: {e}")).unwrap_or_default()
        );
        if v == Verdict::CantHere {
            println!("     this machine can't: its encoder makes {:.0} fps at {w}x{h} alone, less than 1.2 x {fps} (not counted as a failure)", encoder_fps.unwrap_or(0.0));
        }
        if grabs1.0 > grabs0.0 {
            println!("     picture source: {}", grab_summary(grabs1.0 - grabs0.0, grabs1.1 - grabs0.1, wall));
        }
        println!("     stream: {}", d.summary(wall));
    }
    println!("{}", if failed == 0 { "stream test: all passed".to_string() } else { format!("stream test: {failed} failed") });
    failed == 0
}

/// The screen copy alone at the screen's own size, every frame at `fps`, for 2 s: zero-copy
/// where it starts (as the stream would), else through memory.
fn native_copy(mx: i32, my: i32, sw: u32, sh: u32, fps: u32, zero_copy: bool) {
    let o = video::FeedOptions { fps, cursor: true, fit: Some((sw & !1, sh & !1)), every_frame: true };
    #[cfg(zero_copy)]
    let g = if zero_copy { Grabber::start_surfaces((mx, my, sw, sh), None, o).ok() } else { None };
    #[cfg(not(zero_copy))]
    let g: Option<Grabber> = {
        let _ = zero_copy;
        None
    };
    let g = g.unwrap_or_else(|| Grabber::open((mx, my, sw, sh), None, o));
    let t = Instant::now();
    while !g.ready() && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !g.ready() {
        println!("     screen copy alone at the screen's own {sw}x{sh}: {}", g.error().unwrap_or_else(|| "no picture".into()));
        return;
    }
    let (n0, t0, gt0) = (g.captured(), Instant::now(), g.grab_time());
    std::thread::sleep(Duration::from_secs(2));
    let (n1, wall, gt1) = (g.captured(), t0.elapsed().as_secs_f64(), g.grab_time());
    println!(
        "     screen copy alone at the screen's own {sw}x{sh}, {fps} fps, every frame, by {}: {:.1} new pictures a second ({})",
        g.method(),
        (n1 - n0) as f64 / wall,
        grab_summary(gt1.0 - gt0.0, gt1.1 - gt0.1, wall)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_is_720p30_and_full_adds_4k60_and_1080p120() {
        assert_eq!(cases(true), vec![case(720, 30, 3, true)]);
        let full: Vec<(u32, u32)> = cases(false).iter().map(|c| (c.height, c.fps)).collect();
        for want in [(720, 30), (1080, 30), (1440, 60), (2160, 60), (1080, 120)] {
            assert!(full.contains(&want), "{want:?} in {full:?}");
        }
        assert!(cases(false).iter().filter(|c| (c.height, c.fps) == (2160, 60)).all(|c| c.every_frame));
    }

    #[test]
    fn a_screen_smaller_than_the_case_gets_the_test_picture_at_the_cases_size() {
        assert_eq!(case_source(1600, 2160), CaseSource::TestPicture);
        assert_eq!(case_source(1600, 1440), CaseSource::Screen);
        assert_eq!(case_source(2160, 2160), CaseSource::Screen);
        assert_eq!(case_size(2560, 1600, 2160, CaseSource::TestPicture), (3840, 2160));
        assert_eq!(case_size(2560, 1600, 1080, CaseSource::Screen), (1728, 1080));
        assert_eq!(case_size(2560, 1600, 1440, CaseSource::Screen), (2304, 1440));
        assert_eq!(case_size(1920, 1080, 720, CaseSource::TestPicture), (1280, 720));
        // A 4K case on a 4K screen streams the screen at its size.
        assert_eq!(case_size(3840, 2160, 2160, CaseSource::Screen), (3840, 2160));
        assert_eq!(stream_size(1280, 720, 1080), (1280, 720), "never bigger than the screen");
    }

    #[test]
    fn the_full_rate_is_wanted_only_where_the_encoder_has_room() {
        // An encoder at 1.2 times the rate or more: 90 % must arrive.
        assert_eq!(verdict(60, 55.0, Some(72.0), true, true), Verdict::Pass);
        assert_eq!(verdict(60, 50.0, Some(200.0), true, true), Verdict::Fail);
        assert_eq!(verdict(30, 27.0, None, true, true), Verdict::Pass, "speed unknown: the full rate");
        // Too slow an encoder: reported, not failed.
        assert_eq!(verdict(120, 70.0, Some(100.0), true, true), Verdict::CantHere);
        assert_eq!(verdict(60, 10.0, Some(71.9), true, true), Verdict::CantHere);
        // Errors and a slow copy fail whatever the encoder.
        assert_eq!(verdict(60, 60.0, Some(500.0), true, false), Verdict::Fail);
        assert_eq!(verdict(60, 10.0, Some(10.0), true, false), Verdict::Fail);
        assert_eq!(verdict(60, 60.0, Some(500.0), false, true), Verdict::Fail);
        assert_eq!(wanted_copies(30, true), 24.0);
        assert_eq!(wanted_copies(30, false), 0.0, "copied only when the screen changes");
    }

    #[test]
    fn grab_summary_gives_rate_and_time_per_grab() {
        assert_eq!(grab_summary(60, 300_000, 2.0), "30.0 grabs a second, 5.00 ms each (at most 200 a second)");
        assert_eq!(grab_summary(0, 0, 1.0), "0.0 grabs a second, 0.00 ms each (at most 0 a second)");
        assert_eq!(grab_summary(3, 900_000, 0.0), "3000000.0 grabs a second, 300.00 ms each (at most 3 a second)", "no division by zero");
    }
}
