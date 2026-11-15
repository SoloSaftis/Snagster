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
        // Made on the job queue: the window keeps answering, the pictures arrive after.
        let before = std::rc::Rc::new(std::cell::Cell::new(0));
        let b = before.clone();
        s.push(once("File: templates and one-page guide (in the background)", move |a, _| {
            b.set(a.entries.len());
            a.export_guide("page");
            a.combine(false);
            a.combine(true);
            a.make_template(crate::app::Template::Gallery);
            assert_eq!(a.entries.len(), b.get(), "they were made on the window's thread");
        }));
        s.push((
            "File: the made pictures arrive".to_string(),
            Box::new(move |a: &mut SnagApp, _: &egui::Context| a.entries.len() == before.get() + 4),
        ));
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
        s.push(once("File: export several pictures (in the background)", |a, _| {
            let dir = test_dir().join("export");
            let pick = vec![true; a.entries.len()];
            a.export_many(&pick, &dir, crate::settings::ImageFormat::Png);
            assert!(a.status.starts_with("Exporting"), "{}", a.status);
        }));
        s.push((
            "File: the exported pictures arrive".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                if !a.status.starts_with("Exported") {
                    return false;
                }
                assert!(!a.status.contains("failed"), "{}", a.status);
                std::fs::read_dir(test_dir().join("export")).map(|d| d.count()).unwrap_or(0) >= 2
            }),
        ));
        let before = std::rc::Rc::new(std::cell::Cell::new(0));
        let b = before.clone();
        s.push(once("File: open the saved project again (read in the background)", move |a, _| {
            let p = test_dir().join("test.snag");
            b.set(a.entries.len());
            a.open_path(&p);
            assert_eq!(a.entries.len(), b.get(), "it was read on the window's thread");
        }));
        s.push((
            "File: the project arrives".to_string(),
            Box::new(move |a: &mut SnagApp, _: &egui::Context| {
                if a.entries.len() < before.get() + 1 {
                    return false;
                }
                assert!(a.status.starts_with("Opened project"), "{}", a.status);
                true
            }),
        ));
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
            (SettingsTab::Streaming, "Streaming"),
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
        // Per-scene sound: a sound file in this scene (it plays everywhere), shared and back.
        let tone = test_dir().join("tone.wav");
        let file = crate::audio::SoundInput::File { path: tone.to_string_lossy().to_string() };
        let f1 = file.clone();
        s.push(once("Studio: a sound file in this scene", move |a, _| {
            std::fs::write(&tone, crate::wav::tone(1, 440.0)).expect("the test sound couldn't be written");
            let before = a.settings.studio.scene_sounds(0).len();
            super::studio_ui::add_sound(a, crate::studio::SceneSound::new(f1.clone(), "tone.wav"));
            assert_eq!(a.settings.studio.scene_sounds(0).len(), before + 1, "{}", a.status);
            assert!(!a.settings.studio.scene_sounds(1).iter().any(|s| s.input == f1), "only in this scene");
        }));
        let f2 = file.clone();
        s.push((
            "Studio: the sound file plays in the mixer".to_string(),
            Box::new(move |a: &mut SnagApp, _: &egui::Context| a.studio_level(&f2).is_some_and(|l| l > 0.1)),
        ));
        let f3 = file.clone();
        s.push(once("Studio: the sound file in every scene, then this one only, then removed", move |a, _| {
            let st = &mut a.settings.studio;
            let i = st.scenes[0].sounds.iter().position(|s| s.input == f3).expect("not in the scene");
            super::studio_ui::sound_change(st, 0, false, i, super::studio_ui::RowAsk::Share).expect("couldn't share it");
            assert!(st.scene_sounds(1).iter().any(|s| s.input == f3), "the other scene has it now");
            let j = st.shared_sounds.iter().position(|s| s.input == f3).unwrap();
            super::studio_ui::sound_change(st, 0, true, j, super::studio_ui::RowAsk::Share).unwrap();
            let i = st.scenes[0].sounds.iter().position(|s| s.input == f3).unwrap();
            super::studio_ui::sound_change(st, 0, false, i, super::studio_ui::RowAsk::Remove).unwrap();
            assert!(!st.scene_sounds(0).iter().any(|s| s.input == f3));
        }));
        s.push(once("Studio: Ctrl+2 and Ctrl+1 switch scenes", |a, _| {
            super::stream_ui::switch_scene(a, 1);
            assert_eq!(a.settings.studio.current, 1);
            super::stream_ui::switch_scene(a, 0);
            assert_eq!(a.settings.studio.current, 0);
        }));
        s.push(once("Studio: leave", |a, c| {
            a.switch_tab(c, super::ribbon::RibbonTab::Home);
            assert!(a.x.studio.is_none(), "Studio kept running after leaving it");
        }));
        // Live streaming, to a small RTMP server inside this process (never the internet).
        let server: std::rc::Rc<std::cell::RefCell<Option<crate::stream::testserver::TestServer>>> = Default::default();
        s.push(once("Stream: the ribbon tab", |a, c| {
            a.switch_tab(c, super::ribbon::RibbonTab::Stream);
            assert_eq!(a.x.tab, super::ribbon::RibbonTab::Stream);
        }));
        // A second server: the stream goes to both at once (multistreaming).
        let server2: std::rc::Rc<std::cell::RefCell<Option<crate::stream::testserver::TestServer>>> = Default::default();
        let (srv, srv2) = (server.clone(), server2.clone());
        s.push(once("Stream: the Settings page, pointed at two local test servers", move |a, _| {
            let t = crate::stream::testserver::TestServer::start().expect("no local test server");
            let t2 = crate::stream::testserver::TestServer::start().expect("no second local test server");
            let id = a.settings.stream.new_also_id();
            a.settings.stream.also = vec![crate::settings::ExtraService { id, service: crate::stream::Service::Custom, url: t2.url(), on: true }];
            a.set_also_key(id, "ui_test_second_key");
            *srv2.borrow_mut() = Some(t2);
            a.settings_open = true;
            a.settings_tab = SettingsTab::Streaming;
            let st = &mut a.settings.stream;
            st.service = crate::stream::Service::Custom;
            st.url = t.url();
            st.source = crate::stream::Source::Area(0, 0, 320, 240);
            st.max_height = 240;
            st.fps = 30;
            st.kbps = 800;
            st.audio = crate::audio::AudioSource::None;
            st.record_copy = true;
            a.settings.video_folder = test_dir().to_string_lossy().to_string();
            a.set_stream_key("ui_test_stream_key");
            *srv.borrow_mut() = Some(t);
        }));
        s.push(once("Stream: the capture panel's Stream tab", |a, c| {
            a.settings_open = false;
            if !a.panel_open {
                a.enter_panel(c);
            }
            a.settings.panel.tab = 3;
        }));
        s.push(once("Stream: Go live asks first", |a, c| {
            a.toggle_stream(c);
            assert!(a.x.stream.confirm_open, "no confirmation the first time");
            assert_eq!(crate::stream::live::snapshot().phase, crate::stream::live::Phase::Idle, "it went live without asking");
        }));
        s.push(once("Stream: confirm and go live", |a, c| {
            a.x.stream.confirmed = true;
            a.x.stream.confirm_open = false;
            a.start_stream_now(c);
            assert!(crate::stream::live::snapshot().phase != crate::stream::live::Phase::Idle, "{}", a.status);
        }));
        let (srv, srv2) = (server.clone(), server2.clone());
        s.push((
            "Stream: live at both local servers".to_string(),
            Box::new(move |a: &mut SnagApp, _: &egui::Context| {
                let snap = a.x.stream.snap.clone();
                assert!(snap.error.is_none(), "{:?}", snap.error);
                if let Some(st) = &snap.stats {
                    assert!(st.state != crate::stream::State::Failed, "{:?}", st.error);
                }
                let got = srv.borrow().as_ref().map(|t| t.video_messages()).unwrap_or(0);
                let got2 = srv2.borrow().as_ref().map(|t| t.video_messages()).unwrap_or(0);
                if !snap.live() || got < 10 || got2 < 10 {
                    return false;
                }
                assert_eq!(srv.borrow().as_ref().unwrap().publish_name(), "ui_test_stream_key");
                assert_eq!(srv2.borrow().as_ref().unwrap().publish_name(), "ui_test_second_key");
                assert_eq!(snap.stats.as_ref().map(|s| s.outputs.len()), Some(2));
                let i = snap.info.unwrap();
                assert_eq!((i.width, i.height), (320, 240));
                assert_eq!(i.also, vec!["Custom server"]);
                assert!(!i.address.contains("ui_test_stream_key"));
                true
            }),
        ));
        s.push(once("Stream: Stop is instant", |a, c| {
            let t = std::time::Instant::now();
            a.toggle_stream(c);
            assert!(t.elapsed() < std::time::Duration::from_millis(300), "stopping took {:?}", t.elapsed());
            assert!(!crate::stream::live::snapshot().on_air());
        }));
        s.push((
            "Stream: it ends cleanly with a copy on disk".to_string(),
            Box::new(|a: &mut SnagApp, c: &egui::Context| {
                if a.x.stream.snap.phase != crate::stream::live::Phase::Idle {
                    return false;
                }
                let Some(end) = a.x.stream.snap.last.clone() else { return false };
                assert!(end.stats.frames_sent >= 10, "{:?}", end.stats);
                let copy = end.copy.expect("no copy was recorded").expect("the copy failed");
                assert!(copy.video_frames >= 10 && copy.path.exists(), "{copy:?}");
                assert!(a.status.contains(&copy.path.display().to_string()), "{}", a.status);
                a.leave_panel(c);
                true
            }),
        ));
        // Streaming the Studio scenes from the Stream tab: scene buttons, switching while live.
        let (srv, frames_before) = (server.clone(), std::rc::Rc::new(std::cell::Cell::new(0u64)));
        let fb = frames_before.clone();
        s.push(once("Stream: go live with the Studio scenes", move |a, c| {
            a.settings.studio.width = 320;
            a.settings.studio.height = 180;
            a.settings.studio.fps = 30;
            a.settings.stream.source = crate::stream::Source::Studio;
            a.settings.stream.record_copy = false;
            a.switch_tab(c, super::ribbon::RibbonTab::Stream);
            fb.set(srv.borrow().as_ref().map(|t| t.video_messages()).unwrap_or(0));
            a.start_stream_now(c);
            assert!(a.x.studio.is_some(), "Studio didn't start for the stream");
        }));
        let srv = server.clone();
        s.push((
            "Stream: Studio is live; switch scenes while live".to_string(),
            Box::new(move |a: &mut SnagApp, _: &egui::Context| {
                let snap = a.x.stream.snap.clone();
                assert!(snap.error.is_none(), "{:?}", snap.error);
                let got = srv.borrow().as_ref().map(|t| t.video_messages()).unwrap_or(0);
                if !snap.live() || got < frames_before.get() + 10 {
                    return false;
                }
                assert_eq!(snap.info.map(|i| (i.width, i.height)), Some((320, 180)));
                super::stream_ui::switch_scene(a, 1);
                assert_eq!(a.settings.studio.current, 1);
                true
            }),
        ));
        s.push(once("Stream: stop the Studio stream", |a, _| {
            a.stop_stream();
            assert!(!crate::stream::live::snapshot().on_air());
        }));
        s.push((
            "Stream: the Studio stream ends".to_string(),
            Box::new(|a: &mut SnagApp, c: &egui::Context| {
                if a.x.stream.snap.phase != crate::stream::live::Phase::Idle {
                    return false;
                }
                a.settings.stream.source = crate::stream::Source::Screen;
                a.switch_tab(c, super::ribbon::RibbonTab::Home);
                true
            }),
        ));
        s.push(once("Stream: close the test server", move |_, _| {
            *server.borrow_mut() = None;
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
        for tab in 0..4u8 {
            s.push(once(format!("capture panel: tab {}", ["All-in-One", "Image", "Video", "Stream"][tab as usize]), move |a, c| {
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
        // The picker on a made-up screen: a window with a toolbar, a button group and a button.
        s.push(once("picker: a click snaps to the control under the mouse, the wheel steps out", |a, _| {
            use crate::capture::{CutShape, Object, Shot, detect};
            let o = |x, y, w, h, label: &str, children| Object { x, y, width: w, height: h, label: label.into(), children, handle: 0, pid: 0 };
            let objects = vec![o(
                100,
                100,
                600,
                400,
                "Window",
                vec![o(100, 100, 600, 60, "Toolbar", vec![o(110, 110, 200, 40, "Group", vec![o(115, 115, 60, 30, "Button: Save", Vec::new())])])],
            )];
            let image = image::RgbaImage::from_fn(800, 600, |x, y| image::Rgba([(x / 4) as u8, (y / 3) as u8, 90, 255]));
            let shot = Shot { image, origin: (0, 0), objects, title: "Test".into(), app: String::new(), cursor: None, space: detect::Space::default() };
            let mut ov = Overlay::new(OverlayMode::Window, CutShape::Rect, shot);
            let p = pos2(130.0, 130.0);
            assert_eq!(ov.hover_at(p, false, 0).as_deref(), Some("Button: Save"));
            assert_eq!(ov.hover_at(p, false, 1).as_deref(), Some("Group"));
            assert_eq!(ov.hover_at(p, false, 5).as_deref(), Some("Window"));
            assert_eq!(ov.hover_at(p, false, -1).as_deref(), Some("Toolbar"));
            assert_eq!(ov.hover_at(p, true, 0).as_deref(), Some("Window"), "Ctrl takes the whole window");
            assert_eq!(ov.hover_at(pos2(750.0, 550.0), false, 0), None);
            a.overlay = Some(ov);
        }));
        s.push((
            "picker: the frozen screen is drawn".to_string(),
            Box::new(|a: &mut SnagApp, _: &egui::Context| {
                assert!(a.overlay.is_some(), "the picker closed by itself");
                true
            }),
        ));
        s.push(once("picker: close", |a, _| {
            a.overlay = None;
        }));
        s.push(once("scrolling capture: a page with a sticky header is stitched once, the end is found", |_, _| {
            use crate::capture::stitch::{Step, Stitcher};
            // A 2000-row page under a 30-row header that stays put, seen 200 rows at a time.
            let page = |y: u32, x: u32| image::Rgba([(y % 251) as u8, (y / 251 * 30) as u8, (x * 4) as u8, 255]);
            let view = |top: u32| image::RgbaImage::from_fn(60, 200, |x, y| if y < 30 { image::Rgba([255, 0, 0, 255]) } else { page(top + y - 30, x) });
            let mut st = Stitcher::new(view(0), 100_000);
            for top in [80, 160, 240] {
                assert_eq!(st.push(&view(top)), Step::Added(80));
            }
            assert_eq!(st.push(&view(240)), Step::Same);
            assert_eq!(st.push(&view(240)), Step::Same);
            assert_eq!(st.still(), 2, "two unchanged pictures end it");
            let out = st.finish();
            assert_eq!(out.height(), 200 + 240);
            assert_eq!(*out.get_pixel(5, 10), image::Rgba([255, 0, 0, 255]));
            assert_eq!(*out.get_pixel(5, 30 + 300), page(300, 5));
        }));
        s.push(once("Menu capture says so where it isn't available", |a, c| {
            // Windows captures the real menu (skipped here: it takes over the screen).
            if !crate::capture::menus_supported() {
                a.status.clear();
                a.do_action(c, Action::MenuCapture);
                assert_eq!(a.status, crate::i18n::t(crate::capture::MENUS_UNAVAILABLE));
                assert!(a.overlay.is_none());
            }
        }));
        s.push(once("Screen Recording permission: a failed capture says what to do (made-up macOS answer)", |a, c| {
            use crate::capture::{CaptureResult, ScreenAccess};
            let saved = std::mem::take(&mut a.x.access);
            a.x.access.fake = Some(ScreenAccess::NotGranted);
            a.ask_screen_access_once();
            assert!(a.x.access.asked, "macOS's prompt wasn't asked for");
            // A grab fails the way macOS says it without the permission.
            let fail = |a: &mut SnagApp| {
                a.cap_tx.send(CaptureResult { kind: CaptureKind::Screen, shot: Err("Failed to copy data".into()) }).unwrap();
                a.poll_capture(c);
            };
            fail(a);
            let need = crate::i18n::t(crate::capture::SCREEN_ACCESS_NEEDED);
            assert!(a.status.contains(need), "{}", a.status);
            assert!(!a.status.contains("Failed to copy data"), "{}", a.status);
            assert_eq!(a.x.access.open, Some(ScreenAccess::NotGranted), "no window the first time");
            assert!(a.x.access.status_line.as_deref().is_some_and(|l| a.status.contains(l)), "no settings button in the status line");
            // Closed: the next failure only says it in the status line.
            a.x.access.open = None;
            fail(a);
            assert!(a.status.contains(need));
            assert!(a.x.access.open.is_none(), "the window came back");
            // The check runs (--video-test, --stream-test) skip instead of failing.
            assert_eq!(crate::capture::check_skip(a.screen_access()), Some(crate::capture::NO_ACCESS_SKIP));
            a.x.access = saved;
            a.status.clear();
        }));
        s.push(once("scrolling capture: the settings reach the capture", |a, _| {
            a.settings.scroll_notches = 5;
            a.settings.scroll_max_height = 9000;
            a.settings.scroll_from_top = false;
            let o = a.scroll_options(true, false, crate::capture::detect::Space::default());
            assert!(o.horizontal && !o.manual && !o.from_start);
            assert_eq!((o.notches, o.max_length, o.delay_ms, o.max_steps), (5, 9000, a.settings.scroll_wait_ms, a.settings.scroll_max_steps));
            let d = crate::settings::Settings::default();
            (a.settings.scroll_notches, a.settings.scroll_max_height, a.settings.scroll_from_top) = (d.scroll_notches, d.scroll_max_height, d.scroll_from_top);
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
                let started = std::time::Instant::now();
                let r = catch_unwind(AssertUnwindSafe(|| f(self, ctx)));
                // A step that holds up its frame: says which one, so a slow-frame line in the
                // log can be told apart from the window's own work.
                if started.elapsed() >= super::work::SLOW_FRAME {
                    println!("  slow  {name}: the step itself took {:.1} s", started.elapsed().as_secs_f64());
                }
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
