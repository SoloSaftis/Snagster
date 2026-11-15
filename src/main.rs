//! Snagster: capture your screen, mark it up, copy or save it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod hwreport;
mod wav;
mod doc;
mod effects;
mod export;
mod library;
mod mcp;
mod ocr;
mod windows;
mod emoji;
mod fonts;
mod hotkeys;
mod overlay;
mod panel;
mod tray;
mod settings;
mod video;
mod i18n;
mod look;
mod audio;
mod studio;
mod update;
mod userdirs;
mod mcp_http;
mod render;
mod sample;
mod aac;
mod encode;
mod stream;
mod stream_test;
#[cfg(test)]
mod testalloc;

use eframe::egui;
use std::path::PathBuf;

/// The window icon: a blue tile with white viewfinder corners and a dot.
pub fn icon() -> egui::IconData {
    icon_sized(64)
}

/// The icon drawn at any size (designed on a 64-pixel grid).
pub fn icon_sized(n: u32) -> egui::IconData {
    let k = n as f32 / 64.0;
    let mut rgba = vec![0u8; (n * n * 4) as usize];
    let radius = 14.0 * k;
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded square mask.
            let cx = fx.clamp(radius, n as f32 - radius);
            let cy = fy.clamp(radius, n as f32 - radius);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let inside = (radius - d + 0.5).clamp(0.0, 1.0);
            if inside <= 0.0 {
                continue;
            }
            let mut c = [79u8, 124, 255];
            // Corner brackets.
            let (lo, hi, len, th) = (13.0 * k, 51.0 * k, 13.0 * k, 4.0 * k);
            let near = |v: f32, edge: f32| (v - edge).abs() <= th / 2.0;
            let within = |v: f32| v >= lo - th / 2.0 && v <= hi + th / 2.0;
            let mut white = false;
            for &ex in &[lo, hi] {
                for &ey in &[lo, hi] {
                    let sx = if ex == lo { 1.0 } else { -1.0 };
                    let sy = if ey == lo { 1.0 } else { -1.0 };
                    let along_x = (fx - ex) * sx >= -th / 2.0 && (fx - ex) * sx <= len;
                    let along_y = (fy - ey) * sy >= -th / 2.0 && (fy - ey) * sy <= len;
                    if (near(fy, ey) && along_x) || (near(fx, ex) && along_y) {
                        white = true;
                    }
                }
            }
            if ((fx - 32.0 * k).powi(2) + (fy - 32.0 * k).powi(2)).sqrt() <= 6.5 * k {
                white = true;
            }
            if white && within(fx) && within(fy) {
                c = [255, 255, 255];
            }
            let i = ((y * n + x) * 4) as usize;
            rgba[i..i + 3].copy_from_slice(&c);
            rgba[i + 3] = (inside * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: n, height: n }
}

fn main() -> eframe::Result {
    let all: Vec<String> = std::env::args().skip(1).collect();
    // AI apps: `snagster --mcp` talks MCP over stdin/stdout.
    install_crash_log();
    let s0 = settings::Settings::load(false);
    i18n::set(i18n::resolve(&s0.language));
    if matches!(all.first().map(|s| s.as_str()), Some("--mcp" | "mcp")) {
        mcp::serve_stdio();
        return Ok(());
    }
    // AI apps that connect over HTTP: `snagster mcp-http [--port N] [--token T]`.
    if matches!(all.first().map(|s| s.as_str()), Some("--mcp-http" | "mcp-http")) {
        attach_console();
        let (port, token) = mcp_http_options(&all, s0.mcp_port, &s0.mcp_token);
        if let Err(e) = mcp_http::serve_forever(port, &token) {
            eprintln!("Snagster: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    update::clean_up();
    // Anything else on the command line gets the console it was started from, so it can print.
    if !all.is_empty() {
        attach_console();
    }
    if let Some(code) = mcp::run_cli(&all) {
        std::process::exit(code);
    }
    let mut args = all.into_iter();
    let mut screenshot: Option<PathBuf> = None;
    let mut open: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        if a == "--ui-test" {
            app::UI_TEST.store(true, std::sync::atomic::Ordering::Relaxed);
            continue;
        }
        if a == "--icon-png" {
            // Write the app icon (256 px) for packaging.
            if let Some(out) = args.next() {
                let ic = icon_sized(256);
                if let Some(img) = image::RgbaImage::from_raw(ic.width, ic.height, ic.rgba) {
                    let _ = img.save(out);
                }
            }
            return Ok(());
        }
        if a == "--panel" {
            app::START_PANEL.store(true, std::sync::atomic::Ordering::Relaxed);
            continue;
        }
        if a == "--screenshot" {
            screenshot = args.next().map(PathBuf::from);
        } else if a == "--view" {
            // What --screenshot shows: studio, settings-ai, language, record, or lang:<code>.
            if let (Some(v), Ok(mut slot)) = (args.next(), app::SCREENSHOT_VIEW.lock()) {
                *slot = v;
            }
        } else if a == "--record-test" {
            // Record 640 x 360 of the screen for 5 s at 10 fps; report cost; delete the file.
            let path = std::env::temp_dir().join("snag-record-test.gif");
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (tx, rx) = std::sync::mpsc::channel();
            let cpu0 = cpu_seconds();
            let t0 = std::time::Instant::now();
            capture::spawn_gif_recorder((0, 0, 640, 360), 10, 5, 1280, true, path.clone(), stop, egui::Context::default(), tx);
            match rx.recv() {
                Ok(d) => {
                    let wall = t0.elapsed().as_secs_f64();
                    let cpu = cpu_seconds() - cpu0;
                    let size = std::fs::metadata(&d.path).map(|m| m.len()).unwrap_or(0);
                    let stored = export::gif_frames(&d.path, 0).map(|f| f.len()).unwrap_or(0);
                    println!(
                        "recorded {:.1} s: {} frames captured, {} stored (unchanged ones merged), {} KB, CPU {:.1}% of one core ({:.2} s CPU), {:?}",
                        d.seconds,
                        d.frames,
                        stored,
                        size / 1024,
                        100.0 * cpu / wall,
                        cpu,
                        d.result.err()
                    );
                    let _ = std::fs::remove_file(&d.path);
                }
                Err(e) => println!("recording failed: {e}"),
            }
            return Ok(());
        } else if a == "--video-test" {
            // Record short videos in every format and rate, read each back and report.
            video_test(args.next().as_deref() == Some("quick"));
            return Ok(());
        } else if a == "--hw-report" {
            // Everything about graphics-chip encoding here, in one file (see hwreport.rs).
            let ok = hwreport::run(args.next().map(PathBuf::from));
            std::process::exit(if ok { 0 } else { 1 });
        } else if a == "--stream-test" {
            // Stream the screen into a local test server (127.0.0.1 only) and report the cost.
            let ok = stream_test::run(args.next().as_deref() == Some("quick"));
            std::process::exit(if ok { 0 } else { 1 });
        } else if a == "--record-bench" {
            // 50 frames of 1280 x 720: a typing-sized change in each, then full-screen changes.
            let base = render::compose(&sample::sample_doc());
            let base = image::imageops::resize(&base, 1280, 720, image::imageops::FilterType::Triangle);
            let frames: Vec<image::RgbaImage> = (0..50)
                .map(|i| {
                    let mut f = base.clone();
                    for y in 300..330 {
                        for x in 200 + i * 8..208 + i * 8 {
                            f.put_pixel(x, y, image::Rgba([20, 20, 20, 255]));
                        }
                    }
                    f
                })
                .collect();
            for (label, frames) in [("small changes (typing)", frames.clone()), ("whole screen changing", frames.iter().enumerate().map(|(i, f)| image::imageops::huerotate(f, i as i32 * 7)).collect())] {
                let t = std::time::Instant::now();
                let mut old = Vec::new();
                {
                    let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut old, 20);
                    for f in &frames {
                        let _ = enc.encode_frame(image::Frame::from_parts(f.clone(), 0, 0, image::Delay::from_numer_denom_ms(100, 1)));
                    }
                }
                let old_t = t.elapsed().as_secs_f64();
                let t = std::time::Instant::now();
                let mut new = Vec::new();
                {
                    let mut enc = gif::Encoder::new(&mut new, 1280, 720, &[]).unwrap();
                    let mut prev: Option<&image::RgbaImage> = None;
                    for f in &frames {
                        let r = match prev { None => Some((0, 0, 1280, 720)), Some(p) => capture::changed_rect(p, f) };
                        if let Some((l, tp, w, h)) = r {
                            let mut raw = image::imageops::crop_imm(f, l, tp, w, h).to_image().into_raw();
                            let mut fr = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut raw, 30);
                            fr.left = l as u16;
                            fr.top = tp as u16;
                            fr.dispose = gif::DisposalMethod::Keep;
                            let _ = enc.write_frame(&fr);
                        }
                        prev = Some(f);
                    }
                }
                let new_t = t.elapsed().as_secs_f64();
                println!(
                    "{label}: before {:.2} s / {} KB, now {:.2} s / {} KB ({:.0}x less work)",
                    old_t,
                    old.len() / 1024,
                    new_t,
                    new.len() / 1024,
                    old_t / new_t.max(0.0001)
                );
            }
            return Ok(());
        } else if a == "--hotkey-test" {
            // Register the standard hotkeys, press each one for real, and report.
            hotkeys::self_test(&settings::Settings::default());
            return Ok(());
        } else if a == "--self-test" {
            // Check capture, objects, fonts, emojis and text recognition; prints results only.
            let bright = |img: &image::RgbaImage| {
                let n = (img.width() * img.height()).max(1) as f64;
                img.pixels().map(|p| p.0[0] as f64 + p.0[1] as f64 + p.0[2] as f64).sum::<f64>() / (3.0 * n)
            };
            for m in capture::monitors() {
                println!("monitor {} '{}' {}x{} at {},{} primary={}", m.id, m.name, m.width, m.height, m.x, m.y, m.primary);
            }
            match capture::grab_all_screens() {
                Ok(all) => {
                    println!("all screens: {}x{} brightness {:.1}, app in front: '{}'", all.image.width(), all.image.height(), bright(&all.image), all.app);
                    let wins = capture::list_windows();
                    println!("titled windows: {}", wins.len());
                    for w in wins.iter().take(3) {
                        let crop = capture::crop_desktop(&all.image, all.origin, w.x, w.y, w.width, w.height);
                        println!("  window {}x{} -> crop {}", w.width, w.height, crop.map(|c| format!("{}x{} brightness {:.1}", c.width(), c.height(), bright(&c))).unwrap_or("none".into()));
                    }
                }
                Err(e) => println!("all screens failed: {e}"),
            }
            let objs = capture::objects();
            println!(
                "snap targets: {} windows/menus, {} controls inside them",
                objs.len(),
                objs.iter().map(|o| o.children.len()).sum::<usize>()
            );
            match capture::grab_rect(0, 0, 300, 200) {
                Ok(r) => println!("area grab: {}x{}", r.width(), r.height()),
                Err(e) => println!("area grab failed: {e}"),
            }
            fonts::scan_now();
            let fams = fonts::families().unwrap_or_default();
            println!("installed font families: {}", fams.len());
            for f in ["Arial", "Segoe UI", "Times New Roman", "Consolas", "Comic Sans MS"] {
                println!("  {f}: {}", if fonts::path_of(f, false, false).is_some() { "found" } else { "missing" });
            }
            if let Some(f) = fams.iter().find(|f| f.as_str() == "Arial").or(fams.first()) {
                let w = render::measure("Hello", 32.0, f, false, false);
                println!("  text in '{f}' measures {:.0} x {:.0}", w.x, w.y);
            }
            let ef = emoji::font();
            println!("emojis: {} ({}), in {} groups", ef.count(), if ef.color { "color" } else { "black and white" }, ef.groups.len());
            for ch in ['\u{1F600}', '\u{1F44D}', '\u{2705}', '\u{1F680}'] {
                match emoji::render(ch, 64, [0, 0, 0, 255]) {
                    Some(img) => {
                        let painted = img.pixels().filter(|p| p.0[3] > 0).count();
                        let colors: std::collections::HashSet<[u8; 3]> =
                            img.pixels().filter(|p| p.0[3] > 200).map(|p| [p.0[0] >> 4, p.0[1] >> 4, p.0[2] >> 4]).collect();
                        println!("  U+{:X}: {} painted pixels, {} distinct colors", ch as u32, painted, colors.len());
                    }
                    None => println!("  U+{:X}: not drawn", ch as u32),
                }
            }
            let page = effects::guide_page(
                "Snagster self test",
                &[("Email me at test.person@example.com".into(), image::RgbaImage::from_pixel(400, 60, image::Rgba([255, 255, 255, 255])))],
                900,
            )
            .unwrap();
            {
                let (tx, _rx) = std::sync::mpsc::channel();
                match tray::create(&settings::Settings::default(), tx, || {}) {
                    Ok(_t) => println!("taskbar-corner icon with quick menu: created"),
                    Err(e) => println!("taskbar-corner icon failed: {e}"),
                }
            }
            let dir = std::env::temp_dir().join("snag-self-test");
            let _ = std::fs::create_dir_all(&dir);
            let doc = sample::sample_doc();
            let flat = render::compose(&doc);
            // The default settings (JPEG quality 90), so the results don't depend on this
            // computer's settings.
            let defaults = settings::Settings::default();
            for ext in ["png", "jpg", "bmp", "gif", "webp", "tiff", "pdf", "svg"] {
                let p = dir.join(format!("sample.{ext}"));
                let r = export::save_doc(&doc, "Sample", &p, &defaults);
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                let back = if matches!(ext, "pdf" | "svg") { "n/a".to_string() } else { image::open(&p).map(|i| format!("{}x{}", i.width(), i.height())).unwrap_or_else(|e| e.to_string()) };
                println!("  save {ext}: {:?}, {size} bytes, reopens as {back}", r.err());
            }
            let pp = dir.join("sample.snag");
            let rt = export::save_project(&doc, "Sample", &pp).and_then(|_| export::load_project(&pp));
            println!("  project round trip: {}", rt.map(|(_, d)| format!("{} marks kept", d.shapes.len())).unwrap_or_else(|e| e));
            {
                use eframe::egui::{Color32, Rect, pos2};
                let lifted = doc.lift(Rect::from_min_max(pos2(870.0, 610.0), pos2(1100.0, 674.0)));
                println!("  selection lift: {}", lifted.as_ref().map(|d| format!("{} marks (piece added)", d.shapes.len())).unwrap_or("failed".into()));
                let filled = doc.flood_fill(pos2(20.0, 20.0), Color32::from_rgb(255, 0, 0), 32);
                println!("  fill: {}", filled.map(|d| format!("corner is now {:?}", d.image.get_pixel(5, 5).0)).unwrap_or("nothing".into()));
                let erased = doc.erase(&[pos2(100.0, 100.0), pos2(300.0, 100.0)], 20.0);
                println!("  eraser: alpha on the stroke {}", erased.map(|d| d.image.get_pixel(200, 100).0[3]).unwrap_or(255));
                for m in settings::BlurMode::ALL {
                    let r = doc.redact(Rect::from_min_max(pos2(380.0, 440.0), pos2(800.0, 480.0)), m, 12, Color32::BLACK);
                    println!("  blur {}: {}", m.label(), if r.is_some() { "ok" } else { "failed" });
                }
                let trimmed = effects::trim(&effects::canvas(&flat, 1500, 1000, [255, 255, 255, 255]));
                println!("  canvas 1500x1000 then trim: {}x{}", trimmed.width(), trimmed.height());
                let info = effects::apply(effects::Effect::CaptureInfo, &flat, &defaults.effect_options(effects::info_line(&["Test", "Sample"])));
                println!("  capture info strip: {}x{}", info.width(), info.height());
                let dp = dir.join("guide.docx");
                let r = export::docx("Self test", &[("First".into(), flat.clone()), ("Second".into(), circle_free(&flat))], &dp);
                println!("  word document: {:?}, {} bytes", r.err(), std::fs::metadata(&dp).map(|m| m.len()).unwrap_or(0));
                let pp2 = dir.join("piece.snag");
                let rt2 = lifted.ok_or("no lift".to_string()).and_then(|d| export::save_project(&d, "Piece", &pp2)).and_then(|_| export::load_project(&pp2));
                println!("  project with a picture piece: {}", rt2.map(|(_, d)| format!("{} marks kept", d.shapes.len())).unwrap_or_else(|e| e));
            }
            let circle = render::cut_to_shape(&flat, capture::CutShape::Ellipse, &[]);
            println!("  circle cut: corner alpha {}, center alpha {}", circle.get_pixel(0, 0).0[3], circle.get_pixel(640, 400).0[3]);
            let gp = dir.join("anim.gif");
            let frames: Vec<_> = (0..3).map(|i| (image::imageops::resize(&flat, 320 + i, 200, image::imageops::FilterType::Triangle), 200)).collect();
            let frames = export::letterbox(&frames.into_iter().map(|f| f.0).collect::<Vec<_>>(), 320).into_iter().map(|f| (f, 200)).collect::<Vec<_>>();
            let g = export::write_gif(&gp, &frames).and_then(|_| export::gif_frames(&gp, 0));
            println!("  gif round trip: {}", g.map(|f| format!("{} frames", f.len())).unwrap_or_else(|e| e));
            match ocr::recognize(&page) {
                Ok(lines) => {
                    println!("text recognition read {} lines: {:?}", lines.len(), ocr::plain_text(&lines));
                    // Every kind on purpose (web addresses too), whatever the settings say.
                    let all = ocr::RedactKinds::from_names(&ocr::RedactKinds::NAMES).expect("the kinds' own names");
                    let found = ocr::redact(&doc::Doc::new(page.clone()), &lines, all, defaults.blur_block).map(|(_, f)| f).unwrap_or_default();
                    println!("smart redact found: {found:?}");
                }
                Err(e) => println!("text recognition failed: {e}"),
            }
            return Ok(());
        } else if a == "--export-sample" {
            // Save the marked-up sample picture exactly as Save would, then quit.
            let Some(out) = args.next() else { return Ok(()) };
            if let Err(e) = render::compose(&sample::sample_doc()).save(&out) {
                eprintln!("couldn't save {out}: {e}");
            }
            return Ok(());
        } else if !a.starts_with("--") {
            open = Some(PathBuf::from(a));
        }
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Snagster")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([900.0, 560.0])
        .with_icon(icon());
    if app::START_PANEL.load(std::sync::atomic::Ordering::Relaxed) {
        viewport = viewport.with_min_inner_size(panel::PANEL_SIZE).with_inner_size(panel::PANEL_SIZE).with_decorations(false);
    }
    let quiet = screenshot.is_some() || app::UI_TEST.load(std::sync::atomic::Ordering::Relaxed);
    if quiet {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    if screenshot.is_some() {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }

    eframe::run_native(
        "Snagster",
        options,
        Box::new(move |cc| Ok(Box::new(app::SnagApp::new(cc, screenshot, open)))),
    )
}

/// The word after `name` on the command line (`--port 8080` gives "8080").
fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

/// Port and token for `mcp-http`: `--port N` and `--token T`, else the saved settings.
fn mcp_http_options(args: &[String], port: u16, token: &str) -> (u16, String) {
    let port = flag_value(args, "--port").and_then(|p| p.parse().ok()).unwrap_or(port);
    let token = flag_value(args, "--token").unwrap_or_else(|| token.to_string());
    (port, token)
}

fn circle_free(img: &image::RgbaImage) -> image::RgbaImage {
    effects::trim(img)
}

/// CPU time this process has used so far, in seconds.
/// `--video-test`: encoders on this computer, then 3-second recordings in each format and rate,
/// each decoded again to count frames and check the picture is the right way up.
fn video_test(quick: bool) {
    use video::{VideoFormat, VideoOpts};
    println!("Video encoders on this computer:");
    for e in video::encoders() {
        println!("  {:5} {:9} {}{}", e.codec, if e.hardware { "hardware" } else { "software" }, e.name, if e.vendor.is_empty() { String::new() } else { format!(" ({})", e.vendor) });
    }
    // The encoders that hand back frames (live streaming; recording off Windows) and their speed.
    let (lines, encoders_failed) = video::encoder_report(quick);
    for l in lines {
        println!("{l}");
    }
    let (mx, my, sw, sh) = capture::monitors().first().map(|m| (m.x, m.y, m.width, m.height)).unwrap_or((0, 0, 1280, 720));
    let rect = (mx, my, sw.min(1920), sh.min(1080));
    use audio::AudioSource as A;
    let mut cases = vec![(VideoFormat::Mp4, 30, true, A::None), (VideoFormat::Mp4, 60, true, A::System), (VideoFormat::Mp4, 120, true, A::None)];
    if !quick {
        cases.extend([
            (VideoFormat::Mp4, 1, true, A::None),
            (VideoFormat::Hevc, 60, true, A::System),
            (VideoFormat::Wmv, 30, true, A::None),
            (VideoFormat::Mp4, 30, false, A::None),
        ]);
        if !audio::microphones().is_empty() {
            cases.push((VideoFormat::Mp4, 30, true, A::Both));
        }
    }
    let mut failed = encoders_failed;
    // Without screen access (macOS without the permission or a logged-in desktop) the
    // recordings are skipped, not failed; the encoder checks above still count.
    let access = capture::screen_access();
    for (format, fps, hardware, sound) in cases {
        if sound != A::None && !A::supported() {
            continue;
        }
        if !format.available() {
            continue;
        }
        if let Some(skip) = capture::check_skip(access) {
            println!("{skip}: {} {fps} fps", format.label());
            continue;
        }
        let secs = if fps == 1 { 5 } else { 3 };
        let path = std::env::temp_dir().join(format!("snagster-video-test-{fps}-{}-{}.{}", format.ext(), if hardware { "hw" } else { "sw" }, format.ext()));
        let mut opts = VideoOpts::screen(rect, fps, format, path.clone());
        opts.hardware = hardware;
        opts.max_s = secs;
        opts.audio = sound;
        let stop = std::sync::atomic::AtomicBool::new(false);
        let pause = std::sync::atomic::AtomicBool::new(false);
        let before = capture::grab_rect(rect.0, rect.1, rect.2 & !1, rect.3 & !1).ok();
        let cpu0 = cpu_seconds();
        let t0 = std::time::Instant::now();
        let d = video::record(&opts, &stop, &pause);
        let wall = t0.elapsed().as_secs_f64();
        let cpu = cpu_seconds() - cpu0;
        let label = format!("{} {}x{} {fps} fps{}{}", format.label(), d.width, d.height, if hardware { "" } else { " (software asked)" }, if sound == A::None { String::new() } else { format!(", sound: {}", sound.label()) });
        if let Err(e) = &d.result {
            println!("FAIL {label}: {e}");
            failed += 1;
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match video::probe(&path) {
            Ok(p) => {
                let expect = fps as u64 * secs as u64;
                let ok_count = p.frames + 2 >= expect * 9 / 10 && p.frames <= expect + 2;
                // Compare the first frame with the screen now, and with the screen upside down.
                let orient = p.first.as_ref().and_then(|f| {
                    let now = before.clone().filter(|b| b.dimensions() == (p.width, p.height))?;
                    let diff = |a: &image::RgbaImage, b: &image::RgbaImage| {
                        a.pixels().zip(b.pixels()).step_by(13).map(|(x, y)| (0..3).map(|c| (x.0[c] as i32 - y.0[c] as i32).unsigned_abs() as u64).sum::<u64>()).sum::<u64>()
                    };
                    let flipped = image::imageops::flip_vertical(&now);
                    if std::env::var_os("SNAGSTER_VIDEO_DUMP").is_some() {
                        let _ = f.save(std::env::temp_dir().join("snagster-video-first.png"));
                        let _ = now.save(std::env::temp_dir().join("snagster-video-before.png"));
                    }
                    Some(diff(f, &now) <= diff(f, &flipped))
                });
                let ok = ok_count && orient != Some(false) && p.width == d.width && p.height == d.height && (sound == A::None) != p.audio;
                if !ok {
                    failed += 1;
                }
                println!(
                    "{} {label}: {} frames written, {} in the file (want ~{expect}), {:.2} s long, {} KB, {} via {}, CPU {:.1}% of one core, picture {}{}",
                    if ok { "ok  " } else { "FAIL" },
                    d.frames,
                    p.frames,
                    p.seconds,
                    size / 1024,
                    d.encoder,
                    d.capture,
                    100.0 * cpu / wall,
                    match orient {
                        Some(true) => "upright",
                        Some(false) => "UPSIDE DOWN",
                        None => "not checked",
                    },
                    if p.audio { ", with a sound track" } else { "" }
                );
            }
            Err(e) => {
                failed += 1;
                println!("FAIL {label}: couldn't read it back: {e}");
            }
        }
        if std::env::var_os("SNAGSTER_VIDEO_KEEP").is_none() {
            let _ = std::fs::remove_file(&path);
        }
    }
    println!("{}", if failed == 0 { "video test: all passed".to_string() } else { format!("video test: {failed} failed") });
}

/// Where crash reports go: Snagster's settings folder (crash.log, newest last, kept small).
pub fn crash_log_path() -> Option<PathBuf> {
    settings::settings_path().and_then(|p| p.parent().map(|d| d.join("crash.log")))
}

/// A panic anywhere writes what happened and where to crash.log before the usual message, so a
/// problem can be found again. Background threads that panic don't take the window down.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let line = format!(
            "{} Snagster {} ({} {}) thread '{}': {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            thread,
            info
        );
        append_crash_log(&line);
        default(info);
    }));
}

/// Add a line to crash.log (panics, and frames so slow that Windows calls the window
/// "Not responding").
pub fn append_crash_log(line: &str) {
    if let Some(p) = crash_log_path() {
        append_log_line(&p, line, 256 * 1024);
    }
}

/// Append `line` to the file at `p`, starting it again when it is bigger than `limit` bytes.
fn append_log_line(p: &std::path::Path, line: &str, limit: u64) {
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(p).map(|m| m.len() > limit).unwrap_or(false) {
        let _ = std::fs::remove_file(p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

fn cpu_seconds() -> f64 {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) != 0 {
            let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
            return t(k) + t(u);
        }
    }
    #[cfg(unix)]
    {
        // getrusage(RUSAGE_SELF): user then system time, each a timeval (seconds as a 64-bit
        // number, then microseconds; on macOS those are 32-bit in an 8-byte slot, so only the
        // low half is read, which works on both little-endian systems).
        unsafe extern "C" {
            fn getrusage(who: i32, usage: *mut [i64; 18]) -> i32;
        }
        let mut r = [0i64; 18];
        if unsafe { getrusage(0, &mut r) } == 0 {
            let tv = |s: i64, us: i64| s as f64 + (us as u32 as i32) as f64 / 1e6;
            return tv(r[0], r[1]) + tv(r[2], r[3]);
        }
    }
    0.0
}

/// The window build has no console of its own; borrow the one it was started from.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(test)]
mod main_tests {
    use super::*;

    /// build.rs puts assets/snagster.ico into Snagster.exe; Windows picks the size it needs
    /// (16 in lists, 24/32 on the taskbar, 48 in Explorer, 256 for large icons).
    #[test]
    fn the_exe_icon_has_every_size_windows_asks_for() {
        let ico = include_bytes!("../assets/snagster.ico");
        let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]);
        assert_eq!((u16_at(0), u16_at(2)), (0, 1), "not an icon file");
        let n = u16_at(4) as usize;
        let mut sizes = Vec::new();
        for k in 0..n {
            let e = 6 + 16 * k;
            let side = if ico[e] == 0 { 256 } else { ico[e] as u32 };
            let (len, off) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
            assert!(off + len <= ico.len(), "entry {k} runs past the end");
            // Each picture is a PNG of the size the entry says.
            let img = image::load_from_memory(&ico[off..off + len]).expect("a picture in the icon can't be read");
            assert_eq!((img.width(), img.height()), (side, side));
            sizes.push(side);
        }
        for want in [16, 24, 32, 48, 256] {
            assert!(sizes.contains(&want), "no {want} px picture in {sizes:?}");
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_value_takes_the_next_word() {
        let a = args(&["mcp-http", "--port", "9000", "--token", "abc"]);
        assert_eq!(flag_value(&a, "--port").as_deref(), Some("9000"));
        assert_eq!(flag_value(&a, "--token").as_deref(), Some("abc"));
        assert_eq!(flag_value(&a, "--other"), None);
        // A flag at the very end has no value.
        assert_eq!(flag_value(&args(&["mcp-http", "--port"]), "--port"), None);
        assert_eq!(flag_value(&[], "--port"), None);
        // The first one counts.
        assert_eq!(flag_value(&args(&["--port", "1", "--port", "2"]), "--port").as_deref(), Some("1"));
    }

    #[test]
    fn mcp_http_uses_the_command_line_then_the_settings() {
        assert_eq!(mcp_http_options(&args(&["mcp-http"]), 45831, "saved"), (45831, "saved".to_string()));
        assert_eq!(mcp_http_options(&args(&["mcp-http", "--port", "8080"]), 45831, "saved"), (8080, "saved".to_string()));
        assert_eq!(mcp_http_options(&args(&["mcp-http", "--token", "t0k"]), 45831, "saved"), (45831, "t0k".to_string()));
        // A port that isn't a number (or doesn't fit) falls back to the saved one.
        assert_eq!(mcp_http_options(&args(&["mcp-http", "--port", "http"]), 45831, "").0, 45831);
        assert_eq!(mcp_http_options(&args(&["mcp-http", "--port", "70000"]), 45831, "").0, 45831);
    }

    #[test]
    fn icon_has_the_asked_size() {
        for n in [16, 32, 64, 256] {
            let ic = icon_sized(n);
            assert_eq!((ic.width, ic.height), (n, n));
            assert_eq!(ic.rgba.len(), (n * n * 4) as usize);
        }
        let ic = icon();
        assert_eq!((ic.width, ic.height), (64, 64));
    }

    #[test]
    fn icon_is_a_rounded_blue_tile_with_a_white_dot() {
        let ic = icon_sized(64);
        let px = |x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            [ic.rgba[i], ic.rgba[i + 1], ic.rgba[i + 2], ic.rgba[i + 3]]
        };
        // Rounded corners are see-through.
        assert_eq!(px(0, 0)[3], 0);
        assert_eq!(px(63, 63)[3], 0);
        // The middle of an edge is solid blue.
        assert_eq!(px(32, 2), [79, 124, 255, 255]);
        // The centre dot and a bracket corner are white.
        assert_eq!(px(32, 32), [255, 255, 255, 255]);
        assert_eq!(px(13, 13), [255, 255, 255, 255]);
    }

    #[test]
    fn icon_looks_the_same_at_any_size() {
        // The 256 px icon scaled down matches the 64 px one at its main points.
        let big = icon_sized(256);
        let small = icon_sized(64);
        for (x, y) in [(32u32, 32u32), (32, 2), (13, 13), (0, 0)] {
            let i = ((y * 64 + x) * 4) as usize;
            let j = (((y * 4 + 1) * 256 + x * 4 + 1) * 4) as usize;
            assert_eq!(small.rgba[i..i + 3], big.rgba[j..j + 3], "at {x},{y}");
        }
    }

    #[test]
    fn crash_log_sits_next_to_the_settings() {
        if let (Some(log), Some(settings)) = (crash_log_path(), settings::settings_path()) {
            assert_eq!(log.file_name().unwrap(), "crash.log");
            assert_eq!(log.parent(), settings.parent());
        }
    }

    #[test]
    fn log_lines_are_appended_and_the_file_stays_small() {
        let dir = std::env::temp_dir().join(format!("snagster-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("sub").join("crash.log");
        append_log_line(&p, "one\n", 1000);
        append_log_line(&p, "two\n", 1000);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\ntwo\n", "the folder is made and lines are added");
        append_log_line(&p, &"x".repeat(50), 10);
        append_log_line(&p, "fresh\n", 10);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "fresh\n", "a full log starts again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
