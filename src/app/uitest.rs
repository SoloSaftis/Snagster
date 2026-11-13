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
    frames: u32,
    passed: usize,
    failed: Vec<String>,
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
            a.slideshow(c);
            assert!(a.gif_editor.is_some(), "no GIF opened");
        }));
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
                assert!(a.grabbed_text.as_deref().is_some_and(|t| t.contains("Snagster")), "text not read");
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
                assert!(a.status.contains("Blurred"), "nothing redacted: {}", a.status);
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
        ];
        for (tab, name) in tabs {
            s.push(once(format!("Settings tab: {name}"), move |a, _| {
                a.settings_open = true;
                a.settings_tab = tab;
            }));
        }
        s.push(once("close Settings", |a, _| a.settings_open = false));
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
        UiTest { steps: s, at: 0, tries: 0, frames: 0, passed: 0, failed: Vec::new(), dir }
    }

    /// Not run by the test, and why.
    pub(crate) const SKIPPED: &'static [&'static str] = &[
        "Capture buttons and hotkeys (they take over the screen; capture itself is tested by --self-test, hotkeys by --hotkey-test)",
        "Copy / Paste / Paste picture (they would replace what is on your clipboard)",
        "Open, Import, Save as, Export guide, Word (they open file dialogs; the same saving code is tested here)",
        "Print, Email, Open in another program, Show in folder, Open folder (they start other programs)",
        "Move to Recycle Bin (it deletes a file)",
        "Record GIF, Record steps, scrolling (they record the real screen; recording is tested by --record-test)",
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
                    }
                    Ok(false) if t.tries < 600 => t.tries += 1,
                    Ok(false) => {
                        println!("  FAIL  {name}: timed out");
                        t.failed.push(name);
                        t.at += 1;
                        t.tries = 0;
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

fn test_dir() -> PathBuf {
    std::env::temp_dir().join("snagster-ui-test")
}
