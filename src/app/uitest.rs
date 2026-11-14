//! `--ui-test`: drives every button's action in the real window, one step per frame or two,
//! and reports what worked. Things that would take over the screen, the clipboard or start
//! other programs are listed as skipped instead of run.

use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

type Step = (String, Box<dyn Fn(&mut SnagApp, &egui::Context) -> bool>);

pub(crate) struct UiTest {
    steps: Vec<Step>,
    at: usize,
    tries: u32,
    // When the current step first asked to wait.
    waiting: Option<std::time::Instant>,
    frames: u32,
    passed: usize,
    failed: Vec<String>,
    #[allow(dead_code)]
    dir: PathBuf,
}

fn once(name: impl Into<String>, f: impl Fn(&mut SnagApp, &egui::Context) + 'static) -> Step {
    (name.into(), Box::new(move |a, c| {
        f(a, c);
        true
    }))
}

fn cur(a: &SnagApp) -> usize {
    a.current.unwrap_or(0)
}

impl UiTest {
    pub(crate) fn new(dir: PathBuf) -> Self {
        let mut s: Vec<Step> = Vec::new();
        // Tools: pick each, then draw with it the way a drag would.
        for t in Tool::ALL {
            s.push(once(format!("tool button: {}", t.label()), move |a, _| {
                a.tool = t;
            }));
        }
        let drags = [
            Tool::Arrow,
            Tool::Line,
            Tool::Rect,
            Tool::Ellipse,
            Tool::Pen,
            Tool::Highlight,
            Tool::Spotlight,
            Tool::Magnify,
            Tool::Blur,
            Tool::Eraser,
            Tool::CutOut,
            Tool::Selection,
            Tool::Crop,
        ];
        for t in drags {
            s.push(once(format!("draw with: {}", t.label()), move |a, _| {
                a.tool = t;
                let before = a.entries[cur(a)].undo.len();
                let pts = vec![pos2(300.0, 300.0), pos2(340.0, 320.0), pos2(420.0, 380.0)];
                a.finish_draw(pos2(300.0, 300.0), pos2(420.0, 380.0), pts, false);
                assert!(a.entries[cur(a)].undo.len() > before, "nothing was added");
                a.undo();
            }));
        }
        s.push(once("text tool: type and commit", |a, _| {
            let ci = cur(a);
            a.tool = Tool::Text;
            a.start_text(ci, pos2(200.0, 200.0), None);
            if let Some(t) = &mut a.text_edit {
                t.text = "Hello Snagster".into();
            }
            let n = a.entries[ci].doc.shapes.len();
            a.commit_text();
            assert_eq!(a.entries[ci].doc.shapes.len(), n + 1);
        }));
        s.push(once("callout tool: type and commit", |a, _| {
            let ci = cur(a);
            a.tool = Tool::Callout;
            a.start_text_kind(ci, pos2(500.0, 150.0), None, TextKind::Callout { tail: pos2(400.0, 250.0) });
            if let Some(t) = &mut a.text_edit {
                t.text = "Look here".into();
            }
            let n = a.entries[ci].doc.shapes.len();
            a.commit_text();
            assert_eq!(a.entries[ci].doc.shapes.len(), n + 1);
        }));
        s.push(once("step, stamp and fill clicks", |a, _| {
            let ci = cur(a);
            let c = a.color();
            let e = &mut a.entries[ci];
            let n = e.doc.next_step(1);
            e.doc.shapes.push(Shape::Step { center: pos2(100.0, 100.0), n, color: c, radius: 20.0, shadow: true, style: Default::default() });
            e.doc.shapes.push(Shape::Stamp { center: pos2(150.0, 100.0), emoji: "\u{2705}".into(), size: 40.0, color: c });
            e.doc.shapes.push(Shape::Pointer { pos: pos2(200.0, 100.0), size: 30.0 });
            let filled = e.doc.flood_fill(pos2(5.0, 5.0), c, 32);
            assert!(filled.is_some(), "fill did nothing");
        }));
        // Marks: select, copy, paste, cut, resize, recolour, delete, undo, redo.
        s.push(once("select, copy and paste a mark", |a, _| {
            a.tool = Tool::Select;
            a.selected = Some(0);
            a.copy_mark();
            let n = a.entries[cur(a)].doc.shapes.len();
            a.paste_mark();
            assert_eq!(a.entries[cur(a)].doc.shapes.len(), n + 1);
        }));
        s.push(once("cut a mark", |a, _| {
            a.selected = Some(0);
            let n = a.entries[cur(a)].doc.shapes.len();
            a.cut_mark();
            assert_eq!(a.entries[cur(a)].doc.shapes.len(), n - 1);
        }));
        s.push(once("bigger, smaller, recolour, delete", |a, _| {
            a.selected = Some(0);
            a.scale_selected(1.15);
            a.scale_selected(1.0 / 1.15);
            a.set_color(Color32::from_rgb(0, 144, 255));
            a.delete_selected();
        }));
        s.push(once("undo and redo", |a, _| {
            let before = a.entries[cur(a)].doc.shapes.len();
            a.undo();
            a.redo();
            assert_eq!(a.entries[cur(a)].doc.shapes.len(), before);
        }));
        s.push(once("clear all marks, then undo", |a, _| {
            a.clear_marks();
            assert!(a.entries[cur(a)].doc.shapes.is_empty());
            a.undo();
        }));
        // Image menu.
        for e in Effect::ALL {
            s.push(once(format!("Image menu: {}", e.label()), move |a, _| {
                a.apply_effect(e);
                a.undo();
            }));
        }
        s.push(once("Image menu: trim, canvas, exact resize", |a, _| {
            a.image_op("Canvas", |i| crate::effects::canvas(i, i.width() + 80, i.height() + 80, [255, 255, 255, 255]));
            a.image_op("Trim", crate::effects::trim);
            a.image_op("Resize", |i| crate::effects::resize_exact(i, 640, 400));
            let (w, h) = a.entries[cur(a)].doc.image.dimensions();
            assert_eq!((w, h), (640, 400));
            a.undo();
            a.undo();
            a.undo();
        }));
        // File and Tools menus that stay inside Snagster.
        s.push(once("File: new image dialog (create)", |a, _| {
            let n = a.entries.len();
            a.add_entry("Blank".into(), Doc::new(image::RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 255]))));
            assert_eq!(a.entries.len(), n + 1);
        }));
        s.push(once("File: templates and one-page guide", |a, _| {
            let n = a.entries.len();
            a.export_guide("page");
            a.combine(false);
            a.combine(true);
            assert_eq!(a.entries.len(), n + 3);
        }));
        s.push(once("Edit: duplicate and remove picture", |a, _| {
            let n = a.entries.len();
            a.duplicate();
            a.remove_current();
            assert_eq!(a.entries.len(), n);
        }));
        s.push(once("File: rename", |a, _| {
            a.rename_current("Renamed by test");
            assert_eq!(a.entries[cur(a)].name, "Renamed by test");
        }));
        s.push(once("Tools: pin to screen", |a, _| {
            a.pin_current();
            assert!(!a.pins.is_empty());
        }));
        s.push(once("Tools: close pins", |a, _| {
            for p in &mut a.pins {
                p.close = true;
            }
        }));
        s.push(once("Record: slideshow GIF from Recent", |a, c| {
            a.gif_editor = None;
            a.slideshow(c);
        }));
        // The GIF is written on the job queue and read back on a worker, frame by frame.
        s.push((
            "Record: the slideshow opens in the GIF editor".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                let Some(g) = &a.gif_editor else { return false };
                if g.loading.is_some() {
                    return false;
                }
                assert!(!g.frames.is_empty(), "no frames");
                assert_eq!(g.trim, (0, g.frames.len() - 1), "the kept range is every frame");
                true
            }),
        ));
        s.push(once("Record: every video format and frame rate can be chosen", |a, _| {
            for f in crate::video::VideoFormat::ALL {
                a.settings.rec_format = f;
                assert_eq!(a.settings.rec_format.ext(), f.ext());
            }
            for fps in [1, 30, 60, 120] {
                a.settings.video_fps = fps;
            }
            a.settings.rec_format = crate::video::VideoFormat::Mp4;
        }));
        s.push(once("Record: start a 120 fps MP4 video", |a, c| {
            a.settings.video_fps = 120;
            a.settings.video_max_s = 1;
            a.start_video_to(c, (0, 0, 320, 240), Some(test_dir().join("ui-test.mp4")));
            assert!(a.mini == Some(Mini::Video), "the recording bar didn't open");
        }));
        s.push(once("Record: pause and resume", |a, _| {
            a.toggle_pause();
            assert!(a.paused());
            a.toggle_pause();
            assert!(!a.paused());
        }));
        s.push((
            "Record: the video is saved and plays back".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                let Some(v) = a.videos.first() else { return false };
                assert!(v.result.is_ok(), "{:?}", v.result);
                let p = crate::video::probe(&v.path).expect("can't read the video back");
                // The button test checks the video is made and readable; how many frames a second a
                // machine manages is --video-test's job (CI machines encode slowly in software).
                assert!(p.frames >= 10, "only {} frames", p.frames);
                assert_eq!((p.width, p.height), (320, 240));
                assert!(a.mini.is_none(), "the recording bar stayed open");
                a.settings.video_max_s = 0;
                true
            }),
        ));
        s.push(once("Record: GIF editor trim settings", |a, _| {
            if let Some(g) = &mut a.gif_editor {
                g.trim = (0, g.frames.len().saturating_sub(1));
                g.playing = false;
            }
        }));
        // Saving: every format, a project, many at once.
        s.push(once("File: save in every format and as a project", |a, _| {
            let dir = test_dir();
            let i = cur(a);
            for f in crate::settings::ImageFormat::ALL {
                let p = dir.join(format!("test.{}", f.ext()));
                a.save_to(i, p.clone());
                assert!(p.exists(), "{} not written", f.label());
            }
            let p = dir.join("test.snag");
            a.save_to(i, p.clone());
            assert!(p.exists());
        }));
        s.push(once("File: export several pictures", |a, _| {
            let dir = test_dir().join("export");
            let pick = vec![true; a.entries.len()];
            a.export_many(&pick, &dir, crate::settings::ImageFormat::Png);
            assert!(std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) >= 2);
        }));
        s.push(once("File: open the saved project again", |a, _| {
            let p = test_dir().join("test.snag");
            let n = a.entries.len();
            a.open_path(&p);
            assert_eq!(a.entries.len(), n + 1);
        }));
        // After-capture work runs on the job queue and comes back to the window.
        s.push(once("File: quick save on the job queue (start)", |a, _| {
            if let Some(e) = a.entry() {
                e.path = None;
            }
            a.quick_save();
            assert!(!a.saving.is_empty() || a.entry().is_some_and(|e| e.path.is_some()), "nothing was queued");
        }));
        s.push((
            "File: quick save on the job queue (saved)".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                if !a.saving.is_empty() {
                    return false;
                }
                let p = a.entry().and_then(|e| e.path.clone()).expect("the picture doesn't know its file");
                assert!(p.exists(), "{} not written: {}", p.display(), a.status);
                true
            }),
        ));
        s.push(once("Library: a capture is added without reading the folder again (start)", |a, _| {
            a.lib_loaded = true;
            a.lib_items.clear();
            let meta = crate::library::Meta { title: "Button test".into(), kind: "Area".into(), ..Default::default() };
            a.add_to_library(|| image::RgbaImage::from_pixel(64, 48, image::Rgba([200, 30, 30, 255])), meta);
        }));
        s.push((
            "Library: a capture is added without reading the folder again (added)".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                let Some(it) = a.lib_items.first() else { return false };
                assert_eq!(it.meta.title, "Button test");
                assert!(it.path.exists());
                assert!(a.lib_thumbs.contains_key(&it.path), "no thumbnail");
                true
            }),
        ));
        s.push(once("File: Word document", |a, _| {
            let p = test_dir().join("guide.docx");
            let items = a.guide_items();
            crate::export::docx("Test", &items, &p).unwrap();
        }));
        // Text recognition (waits for the background reader).
        s.push(once("Tools: grab text from this picture (start)", |a, _| {
            let page = crate::effects::guide_page(
                "Snagster button test",
                &[("Call 555 123 4567".into(), image::RgbaImage::from_pixel(300, 40, image::Rgba([255, 255, 255, 255])))],
                800,
            )
            .unwrap();
            a.add_entry("Text page".into(), Doc::new(page.clone()));
            a.run_ocr(OcrPurpose::GrabText, page);
        }));
        s.push((
            "Tools: grab text (result)".into(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                if a.ocr_busy {
                    return false;
                }
                // Text recognition is built into Windows only; elsewhere it must say so, not fail silently.
                if cfg!(windows) {
                    assert!(a.grabbed_text.as_deref().is_some_and(|t| t.contains("Snagster")), "text not read");
                } else {
                    assert!(a.status.contains("Windows"), "no clear message: {}", a.status);
                }
                true
            }),
        ));
        s.push(once("Image: smart redact (start)", |a, _| {
            let i = cur(a);
            let img = crate::render::compose(&a.entries[i].doc);
            a.run_ocr(OcrPurpose::Redact(i), img);
        }));
        s.push((
            "Image: smart redact (result)".into(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                if a.ocr_busy {
                    return false;
                }
                if cfg!(windows) {
                    assert!(a.status.contains("Blurred"), "nothing redacted: {}", a.status);
                } else {
                    assert!(a.status.contains("Windows"), "no clear message: {}", a.status);
                }
                true
            }),
        ));
        s.push(once("Image: simplify (start)", |a, _| {
            let i = cur(a);
            let img = crate::render::compose(&a.entries[i].doc);
            a.run_ocr(OcrPurpose::Simplify(i), img);
        }));
        s.push((
            "Image: simplify (result)".into(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| !a.ocr_busy),
        ));
        // Every window and dialog gets drawn at least once.
        let tabs = [
            (SettingsTab::AfterCapture, "After capture"),
            (SettingsTab::Saving, "Saving & library"),
            (SettingsTab::Capture, "Capture"),
            (SettingsTab::Recording, "Recording"),
            (SettingsTab::Editor, "Editor & themes"),
            (SettingsTab::Effects, "Effects"),
            (SettingsTab::Redact, "Smart redact"),
            (SettingsTab::Hotkeys, "Hotkeys"),
            (SettingsTab::Presets, "Presets"),
            (SettingsTab::Ai, "AI apps"),
            (SettingsTab::Language, "Language & updates"),
        ];
        for (tab, name) in tabs {
            s.push(once(format!("Settings tab: {name}"), move |a, _| {
                a.settings_open = true;
                a.settings_tab = tab;
            }));
        }
        s.push(once("close Settings", |a, _| a.settings_open = false));
        // The ribbon: every tab shows its band.
        for tab in super::ribbon::RibbonTab::ALL {
            if tab == super::ribbon::RibbonTab::Studio {
                continue;
            }
            s.push(once(format!("ribbon tab: {tab:?}"), move |a, c| {
                a.switch_tab(c, tab);
                assert_eq!(a.x.tab, tab);
            }));
        }
        s.push(once("ribbon: fold and unfold", |a, _| {
            a.settings.ribbon_folded = true;
            a.settings.ribbon_folded = false;
        }));
        // Studio mode.
        s.push(once("Studio: open", |a, c| {
            a.settings.video_folder = test_dir().to_string_lossy().to_string();
            a.settings.audio_source = crate::audio::AudioSource::None;
            a.switch_tab(c, super::ribbon::RibbonTab::Studio);
            assert!(a.x.studio.is_some(), "the Studio engine didn't start");
        }));
        s.push((
            "Studio: the preview draws".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| a.x.studio.as_ref().is_some_and(|s| s.preview_ready())),
        ));
        s.push(once("Studio: add a text and a color source, a second scene", |a, _| {
            let (w, h) = (a.settings.studio.width as f32, a.settings.studio.height as f32);
            let scene = &mut a.settings.studio.scenes[0];
            scene.items.insert(0, crate::studio::Item { name: "Back".into(), kind: crate::studio::SourceKind::Color { color: [30, 60, 90, 255] }, x: 0.0, y: 0.0, w, h, visible: true });
            scene.items.push(crate::studio::Item {
                name: "Title".into(),
                kind: crate::studio::SourceKind::Text { text: "Snagster Studio".into(), size: 64.0, color: [255, 255, 255, 255] },
                x: 40.0,
                y: 40.0,
                w: 800.0,
                h: 120.0,
                visible: true,
            });
            let copy = a.settings.studio.scenes[0].clone();
            a.settings.studio.scenes.push(copy);
            a.settings.studio.current = 1;
            a.settings.studio.current = 0;
        }));
        s.push(once("Studio: start recording", |a, c| {
            a.settings.studio.fps = 30;
            super::studio_ui::start(a, c);
            assert!(a.x.studio.as_ref().is_some_and(|s| s.recording()));
        }));
        s.push((
            "Studio: record for a second".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                let t = a.x.studio.as_ref().map(|s| s.started_secs()).unwrap_or(0.0);
                if t < 1.2 {
                    return false;
                }
                super::studio_ui::stop(a);
                true
            }),
        ));
        s.push((
            "Studio: the video is saved".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                let Some(st) = &a.x.studio else { return false };
                let Some(d) = &st.last else { return false };
                assert!(d.result.is_ok(), "{:?}", d.result);
                let p = crate::video::probe(&d.path).expect("the Studio video can't be read back");
                // Made and readable; frame rate is --video-test's job (CI machines have no graphics chip).
                assert!(p.frames >= 5, "only {} frames", p.frames);
                assert_eq!((p.width, p.height), (a.settings.studio.width & !1, a.settings.studio.height & !1));
                true
            }),
        ));
        s.push(once("Studio: leave", |a, c| {
            a.switch_tab(c, super::ribbon::RibbonTab::Home);
            assert!(a.x.studio.is_none(), "Studio kept running after leaving it");
        }));
        // Languages: German, Arabic (right to left), Japanese (the computer's CJK fonts), then automatic.
        for code in ["de", "ar", "ja", "zh-Hant", ""] {
            s.push(once(format!("language: {}", if code.is_empty() { "automatic" } else { code }), move |a, c| {
                a.settings.language = code.to_string();
                a.apply_language(c);
                let want = crate::i18n::resolve(code);
                assert_eq!(crate::i18n::current(), want);
                assert_eq!(c.global_style().right_to_left, crate::i18n::is_rtl(want));
            }));
        }
        s.push(once("language list window", |a, c| {
            a.x.lang_open = true;
            a.language_window(c);
            a.x.lang_open = false;
        }));
        for (name, which) in [("Library", 0), ("GIF recordings", 1), ("Timer", 2), ("Keyboard shortcuts", 3), ("About", 4), ("Grabbed text", 5)] {
            s.push(once(format!("window: {name}"), move |a, _| match which {
                0 => a.library_open = true,
                1 => a.gif_open = true,
                2 => a.timer_open = true,
                3 => a.shortcuts_open = true,
                4 => a.about_open = true,
                _ => a.grabbed_text = Some("Some text".into()),
            }));
        }
        s.push(once("close those windows", |a, _| {
            a.library_open = false;
            a.gif_open = false;
            a.timer_open = false;
            a.shortcuts_open = false;
            a.about_open = false;
            a.grabbed_text = None;
        }));
        for (name, which) in [("New image", 0), ("Resize", 1), ("Canvas", 2), ("Rename", 3), ("Export", 4), ("Getting started", 5), ("What's new", 6)] {
            s.push(once(format!("dialog: {name}"), move |a, _| {
                use crate::windows::Dialog;
                a.dialog = Some(match which {
                    0 => Dialog::new_image(),
                    1 => Dialog::Resize { w: 640, h: 400, keep: true, ratio: 1.6 },
                    2 => Dialog::Canvas { w: 800, h: 600, color: [255, 255, 255, 255] },
                    3 => Dialog::Rename("Name".into()),
                    4 => Dialog::export(a.entries.len(), &a.settings),
                    5 => Dialog::GettingStarted,
                    _ => Dialog::WhatsNew,
                });
            }));
        }
        s.push(once("close dialog", |a, _| a.dialog = None));
        // The capture panel and its tabs.
        for tab in 0..3u8 {
            s.push(once(format!("capture panel: tab {}", ["All-in-One", "Image", "Video"][tab as usize]), move |a, c| {
                if !a.panel_open {
                    a.enter_panel(c);
                }
                a.settings.panel.tab = tab;
            }));
        }
        s.push(once("capture panel: save choices as a preset (+)", |a, _| {
            let n = a.settings.presets.len();
            let p = a.panel_as_preset();
            a.settings.presets.push(p);
            assert_eq!(a.settings.presets.len(), n + 1);
        }));
        s.push(once("capture panel: Open Editor", |a, c| {
            a.leave_panel(c);
            assert!(!a.panel_open);
        }));
        s.push(once("View: hide and show panels, customize toolbar", |a, _| {
            a.settings.show_side_panel = false;
            a.settings.show_tray = false;
            a.settings.hidden_tools.push("Pen".into());
        }));
        s.push(once("View: restore", |a, _| {
            a.settings.show_side_panel = true;
            a.settings.show_tray = true;
            a.settings.hidden_tools.clear();
        }));
        s.push(once("View: zoom buttons", |a, c| {
            a.set_zoom(c, Some(1.25));
            a.set_zoom(c, Some(0.8));
            a.set_zoom(c, Some(0.0));
            a.set_zoom(c, None);
        }));
        UiTest { steps: s, at: 0, tries: 0, waiting: None, frames: 0, passed: 0, failed: Vec::new(), dir }
    }

    /// Not run by the test, and why.
    pub(crate) const SKIPPED: &'static [&'static str] = &[
        "Capture buttons and hotkeys (they take over the screen; capture itself is tested by --self-test, hotkeys by --hotkey-test)",
        "Copy / Paste / Paste picture (they would replace what is on your clipboard)",
        "Open, Import, Save as, Export guide, Word (they open file dialogs; the same saving code is tested here)",
        "Print, Email, Open in another program, Show in folder, Open folder (they start other programs)",
        "Move to Recycle Bin (it deletes a file)",
        "Record GIF, Record steps, scrolling (they record the real screen; GIFs are tested by --record-test, videos by --video-test and the video steps above)",
        "Play last video, Show last video in its folder (they start other programs)",
        "Start with Windows (changes the registry)",
    ];
}

impl SnagApp {
    /// One test step per two frames; prints a report and closes at the end.
    pub(crate) fn ui_test_tick(&mut self, ctx: &egui::Context) {
        let Some(mut t) = self.ui_test.take() else { return };
        ctx.request_repaint();
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        t.frames += 1;
        if t.frames % 2 == 0 {
            if t.at < t.steps.len() {
                let (name, f) = &t.steps[t.at];
                let name = name.clone();
                let r = catch_unwind(AssertUnwindSafe(|| f(self, ctx)));
                match r {
                    Ok(true) => {
                        println!("  ok    {name}");
                        t.passed += 1;
                        t.at += 1;
                        t.tries = 0;
                        t.waiting = None;
                    }
                    Ok(false) if !gave_up(t.tries, t.waiting.get_or_insert_with(std::time::Instant::now).elapsed()) => {
                        t.tries += 1
                    }
                    Ok(false) => {
                        println!("  FAIL  {name}: timed out");
                        t.failed.push(name);
                        t.at += 1;
                        t.tries = 0;
                        t.waiting = None;
                    }
                    Err(e) => {
                        let msg = e
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_default();
                        println!("  FAIL  {name}: {msg}");
                        t.failed.push(name);
                        t.at += 1;
                        t.tries = 0;
                        t.waiting = None;
                    }
                }
            } else {
                println!("\n{} of {} button actions worked.", t.passed, t.steps.len());
                for f in &t.failed {
                    println!("  failed: {f}");
                }
                println!("Not run on purpose:");
                for s in UiTest::SKIPPED {
                    println!("  - {s}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.ui_test = Some(t);
    }
}

/// A waiting step gives up only after both 600 tries and a minute, so a fast machine doesn't wait
/// a minute for nothing and a slow one (software graphics, CI) still gets the time it needs.
fn gave_up(tries: u32, waited: std::time::Duration) -> bool {
    tries >= 600 && waited >= std::time::Duration::from_secs(60)
}

fn test_dir() -> PathBuf {
    std::env::temp_dir().join("snagster-ui-test")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_step_needs_both_many_tries_and_a_minute_before_it_gives_up() {
        use std::time::Duration;
        // Fast machine: 600 tries go by in seconds; keep waiting until the minute is up.
        assert!(!gave_up(600, Duration::from_secs(5)));
        // Slow machine: a minute passes in few tries; keep trying.
        assert!(!gave_up(40, Duration::from_secs(90)));
        assert!(gave_up(600, Duration::from_secs(60)));
        assert!(!gave_up(0, Duration::ZERO));
    }

    #[test]
    fn steps_have_unique_names_and_cover_every_tool() {
        let t = UiTest::new(std::env::temp_dir().join("snagster-ui-test-unit-never-created"));
        assert!(t.steps.len() > Tool::ALL.len());
        let mut names = std::collections::HashSet::new();
        for (name, _) in &t.steps {
            assert!(names.insert(name.as_str()), "two steps are called {name:?}");
        }
        for tool in Tool::ALL {
            assert!(names.contains(format!("tool button: {}", tool.label()).as_str()), "{tool:?}");
        }
        assert_eq!((t.at, t.passed), (0, 0));
        assert!(t.failed.is_empty());
    }

    #[test]
    fn skipped_list_says_why() {
        assert!(!UiTest::SKIPPED.is_empty());
        for s in UiTest::SKIPPED {
            assert!(s.contains('(') && s.ends_with(')'), "{s}: give the reason in brackets");
        }
    }

    #[test]
    fn the_test_folder_is_in_temp() {
        assert!(test_dir().starts_with(std::env::temp_dir()));
    }

    #[test]
    fn once_steps_always_finish() {
        let (name, f) = once("x", |a, _| a.tool = Tool::Pen);
        let mut a = SnagApp::blank(Settings::default(), false, None);
        assert_eq!(name, "x");
        assert!(f(&mut a, &egui::Context::default()));
        assert_eq!(a.tool, Tool::Pen);
    }
}
