//! The rest of Snagster's commands as MCP tools: more ways to capture, editing, guides,
//! the library, settings, languages, updates, sound devices, Studio scenes and recordings.

use super::{CaptureArgs, Grabbed, Outcome, Tool, Workspace, bool_arg, capture_props, index_arg, link, num, obj, preview, rect_arg, saved, text, with};
use crate::capture::{self, CaptureKind, CutShape};
use crate::doc::Doc;
use crate::settings::{Preset, PresetMode};
use crate::{effects, export, library, ocr, render};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn tools() -> Vec<Tool> {
    let n = |d: &str| json!({"type": "number", "description": d});
    let s = |d: &str| json!({"type": "string", "description": d});
    let b = |d: &str| json!({"type": "boolean", "description": d});
    let rect = || json!({"x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number"}, "height": {"type": "number"}});
    let formats = crate::video::VideoFormat::video_names();
    let mut studio_formats = formats.clone();
    studio_formats.push("gif");
    let scene = || n("Scene number from studio_scenes (default: the current one).");
    vec![
        Tool { name: "capture_active_window", description: "Capture the window in front as the current picture (Windows only).", schema: obj(capture_props(), &[]), edits: true },
        Tool {
            name: "capture_last_area",
            description: "Capture the same desktop area as the last capture_region (or capture_scrolling) in this session again.",
            schema: obj(capture_props(), &[]),
            edits: true,
        },
        Tool {
            name: "capture_scrolling",
            description: "Scroll a desktop area (a web page, a document) and stitch it into one long picture, as the current picture. Keep the mouse still while it runs.",
            schema: obj(with(with(rect(), json!({"horizontal": b("Scroll sideways instead (panorama)."), "max_steps": n("Most scroll steps, default: the setting (40).")})), capture_props()), &["x", "y", "width", "height"]),
            edits: true,
        },
        Tool {
            name: "run_preset",
            description: "Capture with one of the person's presets (see list_presets): its kind of capture, delay, effects, and copy/save steps. Area, shape and scrolling presets need x, y, width and height; fixed-size presets take x and y (default: the top-left of the screen under the mouse).",
            schema: obj(with(with(json!({"number": n("Preset number from list_presets."), "name": s("Or part of the preset's name.")}), rect()), capture_props()), &[]),
            edits: true,
        },
        Tool { name: "pick_color", description: "The colour of one desktop pixel, as #RRGGBB.", schema: obj(json!({"x": {"type": "number"}, "y": {"type": "number"}}), &["x", "y"]), edits: false },
        Tool { name: "read_screen_text", description: "Read the text in a desktop rectangle (Windows only: Windows text recognition), without changing the current picture.", schema: obj(rect(), &["x", "y", "width", "height"]), edits: false },
        Tool {
            name: "simplify",
            description: "Turn every line of text in the current picture into a plain bar, so it shows the layout without the details (Windows only: uses Windows text recognition).",
            schema: obj(json!({}), &[]),
            edits: true,
        },
        Tool {
            name: "quick_save",
            description: "Save the current picture into Snagster's save folder, named by the file-name pattern and in the format from the settings (as the window's Save does).",
            schema: obj(json!({}), &[]),
            edits: false,
        },
        Tool { name: "redo", description: "Redo what undo undid.", schema: obj(json!({}), &[]), edits: true },
        Tool { name: "clear_marks", description: "Remove every mark from the current picture.", schema: obj(json!({}), &[]), edits: true },
        Tool { name: "trim", description: "Cut away flat-coloured edges of the current picture.", schema: obj(json!({}), &[]), edits: true },
        Tool {
            name: "set_canvas",
            description: "Change the canvas size of the current picture (the picture stays in the middle; new space gets the colour).",
            schema: obj(json!({"width": {"type": "number"}, "height": {"type": "number"}, "color": s("#RRGGBB, default white.")}), &["width", "height"]),
            edits: true,
        },
        Tool { name: "paste_from_clipboard", description: "Make the picture on the clipboard the current picture.", schema: obj(json!({}), &[]), edits: true },
        Tool {
            name: "combine_images",
            description: "Put picture files side by side (or top to bottom) into one picture, as the current picture.",
            schema: obj(json!({"paths": {"type": "array", "items": {"type": "string"}, "description": "Full paths, two or more."}, "vertical": b("Top to bottom instead of side by side.")}), &["paths"]),
            edits: true,
        },
        Tool {
            name: "make_guide",
            description: "Make a step-by-step guide from pictures with captions: a PDF, a web page (html), a Word document (docx) or one tall picture (png). An existing file is replaced.",
            schema: obj(
                json!({
                    "title": s("The guide's title."),
                    "steps": {"type": "array", "items": {"type": "object", "properties": {"path": {"type": "string"}, "caption": {"type": "string"}}, "required": ["path"]}},
                    "include_current": b("Add the current picture as the last step."),
                    "path": s("Where to save it; the extension picks the format: .pdf, .html, .docx or .png.")
                }),
                &["path"],
            ),
            edits: false,
        },
        Tool {
            name: "library_add",
            description: "Keep the current picture in Snagster's library (searchable later), with a title and tags.",
            schema: obj(json!({"title": s("Default: the picture's name."), "tags": {"type": "array", "items": {"type": "string"}}}), &[]),
            edits: false,
        },
        Tool {
            name: "library_tag",
            description: "Change a library picture's title or tags (by the path library_search gives): set all tags, or add and remove some.",
            schema: obj(
                json!({
                    "path": s("The picture's path from library_search."),
                    "title": s("New title."),
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Replace all tags with these."},
                    "add": {"type": "array", "items": {"type": "string"}, "description": "Tags to add."},
                    "remove": {"type": "array", "items": {"type": "string"}, "description": "Tags to take away."}
                }),
                &["path"],
            ),
            edits: false,
        },
        Tool {
            name: "library_delete",
            description: "Delete a picture from Snagster's library for good (by the path library_search gives). Only pictures in the library can be deleted.",
            schema: obj(json!({"path": s("The picture's path from library_search.")}), &["path"]),
            edits: false,
        },
        Tool { name: "list_recordings", description: "The videos and GIFs in Snagster's recordings folder, newest first.", schema: obj(json!({"limit": n("Default 20.")}), &[]), edits: false },
        Tool { name: "video_info", description: "Read a video file back: size, frames, length (MP4, WMV) or a GIF's frames and length.", schema: obj(json!({"path": s("Full path.")}), &["path"]), edits: false },
        Tool { name: "get_settings", description: "Snagster's settings as JSON (or one of them, by key).", schema: obj(json!({"key": s("One setting's name, e.g. video_fps.")}), &[]), edits: false },
        Tool {
            name: "set_setting",
            description: "Change one of Snagster's settings (see get_settings for names and current values). Saved at once; a running Snagster window picks it up when it next saves or restarts.",
            schema: obj(json!({"key": s("The setting's name, e.g. video_fps, rec_format, audio_source, language, save_folder."), "value": {"description": "The new value, as JSON (number, text, true/false...)."}}), &["key", "value"]),
            edits: false,
        },
        Tool { name: "list_languages", description: "The 86 languages Snagster's interface speaks: code, English name and own name.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "set_language", description: "Set Snagster's interface language by code (from list_languages), or \"auto\" for the computer's.", schema: obj(json!({"code": s("e.g. de, ja, pt-BR, auto.")}), &["code"]), edits: false },
        Tool { name: "app_info", description: "Snagster's version, system, folders, language, video encoders, sound support and how AI apps can connect.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "check_for_update", description: "Ask GitHub whether a newer Snagster is out.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "list_audio_devices", description: "The microphones that can be recorded (ids for record_video's microphone), and whether sound recording works here.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "open_in_snagster", description: "Open the current picture (with its marks, still editable) in the Snagster window.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "list_presets", description: "The capture presets the person set up (run one with run_preset).", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "studio_scenes", description: "Studio mode's scenes and their sources (screens, windows, areas, pictures, text, colours) with positions.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "studio_add_scene", description: "Add an empty Studio scene.", schema: obj(json!({"name": s("Its name (default: Scene N).")}), &[]), edits: false },
        Tool { name: "studio_remove_scene", description: "Remove a Studio scene and its sources (the last scene stays).", schema: obj(json!({"scene": n("Scene number from studio_scenes.")}), &["scene"]), edits: false },
        Tool { name: "studio_set_current", description: "Make a scene the current one (what Studio shows and records).", schema: obj(json!({"scene": n("Scene number from studio_scenes.")}), &["scene"]), edits: false },
        Tool {
            name: "studio_update_source",
            description: "Change a source in a Studio scene (by its number from studio_scenes): its name, place and size on the canvas, whether it shows, and what it shows (screen, window title, desktop area, picture, text, colour).",
            schema: obj(
                json!({
                    "scene": scene(),
                    "index": n("Source number."),
                    "name": s("Its name in the list."),
                    "left": n("Canvas x."), "top": n("Canvas y."), "w": n("Canvas width."), "h": n("Canvas height."),
                    "visible": b("Shown or hidden."),
                    "screen": n("Screen number (screen sources)."),
                    "title": s("Part of the window title (window sources)."),
                    "x": n("Desktop x (area sources)."), "y": n("Desktop y."), "width": n("Desktop width."), "height": n("Desktop height."),
                    "path": s("Picture file (picture sources)."),
                    "text": s("Text (text sources)."), "size": n("Text size."), "color": s("#RRGGBB or #RRGGBBAA (text and colour sources).")
                }),
                &["index"],
            ),
            edits: false,
        },
        Tool {
            name: "studio_add_source",
            description: "Add a source to a Studio scene. kind: screen (index), window (title), area (x, y, width, height on the desktop), picture (path), text (text, size, color), color (color). Placed at (left, top) with (w, h) on the canvas (default: the whole canvas).",
            schema: obj(
                json!({
                    "scene": n("Scene number from studio_scenes (default: the current one)."),
                    "kind": {"type": "string", "enum": ["screen", "window", "area", "picture", "text", "color"]},
                    "name": s("Its name in the list."),
                    "index": n("Screen number for kind screen (0 = first)."),
                    "title": s("Part of the window title for kind window."),
                    "x": n("Desktop x for kind area."), "y": n("Desktop y for kind area."), "width": n("Desktop width for kind area."), "height": n("Desktop height for kind area."),
                    "path": s("Picture file for kind picture."),
                    "text": s("For kind text."), "size": n("Text size."), "color": s("#RRGGBB or #RRGGBBAA."),
                    "left": n("Canvas x."), "top": n("Canvas y."), "w": n("Canvas width."), "h": n("Canvas height.")
                }),
                &["kind"],
            ),
            edits: false,
        },
        Tool {
            name: "studio_remove_source",
            description: "Remove a source from a Studio scene by its number (from studio_scenes).",
            schema: obj(json!({"scene": n("Scene number (default: the current one)."), "index": n("Source number.")}), &["index"]),
            edits: false,
        },
        Tool {
            name: "studio_record",
            description: "Record a Studio scene (all its sources composed together, with Studio's sound mixer) to a video file for some seconds. Format, quality, sound and encoder come from the settings unless given; Studio records video, so gif records MP4 (as the Studio window does).",
            schema: obj(
                json!({
                    "seconds": n("1-3600."),
                    "scene": scene(),
                    "fps": n("1-120, default Studio's setting."),
                    "format": {"type": "string", "enum": studio_formats, "description": "Default: the setting (MP4 when it is GIF)."},
                    "audio": {"type": "string", "enum": ["none", "system", "microphone", "both"], "description": "Default: the setting (sound is recorded on Windows only)."},
                    "microphone": s("Microphone id from list_audio_devices (default: the setting)."),
                    "path": s("Where to save it; default a new file in the recordings folder (the video_folder setting).")
                }),
                &["seconds"],
            ),
            edits: false,
        },
    ]
}

/// Whether two paths name the same file.
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn str_list(a: &Value, k: &str) -> Option<Vec<String>> {
    a.get(k).and_then(Value::as_array).map(|v| v.iter().filter_map(|t| t.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect())
}

/// Studio's format: a video format this system makes; GIF becomes MP4 (and says so).
fn studio_format(f: &str) -> Result<(crate::video::VideoFormat, &'static str), String> {
    use crate::video::VideoFormat;
    match VideoFormat::parse(f) {
        Some(VideoFormat::Gif) => Ok((VideoFormat::Mp4, " Studio records video, so GIF became MP4 (as in the Studio window).")),
        Some(_) => Ok((super::video_format_arg(f)?, "")),
        None => Err(format!("format is one of {}, gif", VideoFormat::video_names().join(", "))),
    }
}

/// What a preset captures, headless.
#[derive(Debug, PartialEq)]
enum PresetSource {
    Screen,
    AllScreens,
    ActiveWindow,
    /// A desktop area from the arguments.
    Area,
    /// A fixed-size area at the arguments' x, y.
    Fixed(u32, u32),
    Scrolling,
}

fn preset_plan(p: &Preset) -> (PresetSource, Option<CutShape>) {
    match p.mode {
        PresetMode::Screen => (PresetSource::Screen, None),
        PresetMode::AllScreens => (PresetSource::AllScreens, None),
        PresetMode::Window => (PresetSource::ActiveWindow, None),
        PresetMode::Area | PresetMode::Freehand => (PresetSource::Area, None),
        PresetMode::Circle => (PresetSource::Area, Some(CutShape::Ellipse)),
        PresetMode::Star => (PresetSource::Area, Some(CutShape::Star)),
        PresetMode::Heart => (PresetSource::Area, Some(CutShape::Heart)),
        PresetMode::FixedSize => (PresetSource::Fixed(p.width.max(8), p.height.max(8)), None),
        PresetMode::Scrolling => (PresetSource::Scrolling, None),
    }
}

/// Change a Studio source as studio_update_source asks; what changed.
fn update_item(item: &mut crate::studio::Item, a: &Value) -> Result<Vec<&'static str>, String> {
    use crate::studio::SourceKind;
    let mut what = Vec::new();
    if let Some(n) = str_arg(a, "name") {
        item.name = n.to_string();
        what.push("name");
    }
    for (k, slot) in [("left", &mut item.x), ("top", &mut item.y)] {
        if let Some(v) = num(a, k) {
            *slot = v as f32;
            what.push("place");
        }
    }
    for (k, slot) in [("w", &mut item.w), ("h", &mut item.h)] {
        if let Some(v) = num(a, k) {
            if v < 1.0 {
                return Err(format!("{k} must be 1 or more"));
            }
            *slot = v as f32;
            what.push("size");
        }
    }
    if let Some(v) = bool_arg(a, "visible") {
        item.visible = v;
        what.push("shown");
    }
    let label = item.kind.label().to_lowercase();
    let wrong = |k: &str| format!("a {label} source has no {k}");
    let wants = |keys: &[&str]| keys.iter().any(|k| a.get(*k).is_some());
    if wants(&["screen"]) {
        let e = wrong("screen");
        let SourceKind::Display { index } = &mut item.kind else { return Err(e) };
        *index = index_arg(a, "screen")?;
        what.push("screen");
    }
    if wants(&["title"]) {
        let e = wrong("title");
        let SourceKind::Window { title } = &mut item.kind else { return Err(e) };
        *title = str_arg(a, "title").ok_or("title is text")?.to_string();
        what.push("window");
    }
    if wants(&["x", "y", "width", "height"]) {
        let e = wrong("desktop area");
        let r = rect_arg(a)?;
        let SourceKind::Area { x, y, w, h } = &mut item.kind else { return Err(e) };
        (*x, *y, *w, *h) = (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32);
        what.push("area");
    }
    if wants(&["path"]) {
        let e = wrong("picture");
        let SourceKind::Image { path } = &mut item.kind else { return Err(e) };
        *path = str_arg(a, "path").ok_or("path is text")?.to_string();
        what.push("picture");
    }
    if wants(&["text", "size", "color"]) {
        let e = wrong("text or colour");
        match &mut item.kind {
            SourceKind::Text { text, size, color } => {
                if let Some(t) = str_arg(a, "text") {
                    *text = t.to_string();
                }
                if let Some(s) = num(a, "size") {
                    *size = s.clamp(4.0, 1000.0) as f32;
                }
                *color = color4(str_arg(a, "color"), *color);
            }
            SourceKind::Color { color } if a.get("text").is_none() && a.get("size").is_none() => *color = color4(str_arg(a, "color"), *color),
            _ => return Err(e),
        }
        what.push("look");
    }
    what.dedup();
    if what.is_empty() {
        return Err("nothing to change: give name, left, top, w, h, visible or what the source shows".into());
    }
    Ok(what)
}

fn json_text(v: &impl serde::Serialize) -> Value {
    text(serde_json::to_string_pretty(v).unwrap_or_default())
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn color4(s: Option<&str>, default: [u8; 4]) -> [u8; 4] {
    let Some(hex) = s.map(|s| s.trim().trim_start_matches('#')) else { return default };
    let v = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
    match hex.len() {
        6 => [v(0), v(2), v(4), 255],
        8 => [v(0), v(2), v(4), v(6)],
        _ => {
            let c = super::parse_color(s);
            [c.r(), c.g(), c.b(), 255]
        }
    }
}

fn scene_index(a: &Value, st: &crate::studio::StudioSettings) -> Result<usize, String> {
    let i = match num(a, "scene") {
        Some(_) => index_arg(a, "scene")?,
        None => st.current,
    };
    if i >= st.scenes.len() {
        return Err(format!("there is no scene {i} (see studio_scenes)"));
    }
    Ok(i)
}

impl Workspace {
    pub(super) fn call_more(&mut self, name: &str, a: &Value) -> Outcome {
        match name {
            "capture_active_window" => {
                let ca = CaptureArgs::parse(a)?;
                self.wait(ca.delay_s)?;
                let shot = capture::grab(&CaptureKind::ActiveWindow, (0, 0))?;
                self.finish_capture(Grabbed::from_shot(shot, None), "Window", &ca, &[])
            }
            "capture_last_area" => {
                let ca = CaptureArgs::parse(a)?;
                let (x, y, w, h) = self.last_area.ok_or("No area was captured yet in this session: use capture_region first.")?;
                self.wait(ca.delay_s)?;
                let shot = capture::grab(&CaptureKind::LastRegion(x, y, w, h), (0, 0))?;
                self.finish_capture(Grabbed::from_shot(shot, None), "Area", &ca, &[])
            }
            "capture_scrolling" => {
                let r = rect_arg(a)?;
                let ca = CaptureArgs::parse(a)?;
                let horizontal = a.get("horizontal").and_then(Value::as_bool).unwrap_or(false);
                let s = self.settings();
                let steps = num(a, "max_steps").unwrap_or(s.scroll_max_steps as f64).clamp(1.0, 200.0) as u32;
                let area = (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32);
                self.wait(ca.delay_s)?;
                let img = self.ticking(None, "Scrolling and stitching", || capture::scroll_capture(area, horizontal, steps, s.scroll_wait_ms, self.stop.clone(), false))?;
                self.last_area = Some(area);
                let title = if horizontal { "Panorama" } else { "Scrolling capture" };
                let g = Grabbed { image: img, origin: (area.0, area.1), cursor: None, title: title.into(), app: capture::foreground_info().1 };
                self.finish_capture(g, "Area", &ca, &[])
            }
            "run_preset" => self.run_preset(a),
            "simplify" => {
                let d = self.doc()?.clone();
                let flat = render::compose(&d);
                let lines = ocr::recognize(&flat)?;
                if lines.is_empty() {
                    return Ok(vec![text("No text was found.")]);
                }
                let out = ocr::simplify(&flat, &lines);
                self.change(|_| Ok(Doc::new(out)))?;
                Ok(vec![text(format!("Simplified: {} lines of text became bars.", lines.len()))])
            }
            "quick_save" => self.quick_save(),
            "pick_color" => {
                let (x, y) = (super::need(a, "x")? as i32, super::need(a, "y")? as i32);
                let img = capture::grab_rect(x, y, 1, 1)?;
                let p = img.get_pixel(0, 0);
                Ok(vec![text(format!("#{:02X}{:02X}{:02X}", p[0], p[1], p[2]))])
            }
            "read_screen_text" => {
                let r = rect_arg(a)?;
                let img = capture::grab_rect(r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32)?;
                let lines = ocr::recognize(&img)?;
                Ok(vec![text(ocr::plain_text(&lines))])
            }
            "redo" => {
                let next = self.redo.pop().ok_or("nothing to redo")?;
                if let Some(cur) = self.doc.replace(next) {
                    self.undo.push(cur);
                }
                Ok(vec![text("Redone.")])
            }
            "clear_marks" => {
                self.change(|d| {
                    let mut d = d.clone();
                    d.shapes.clear();
                    Ok(d)
                })?;
                Ok(vec![text("Every mark removed.")])
            }
            "trim" => {
                self.flatten_with(effects::trim)?;
                let (w, h) = self.doc()?.image.dimensions();
                Ok(vec![text(format!("Trimmed to {w} x {h}."))])
            }
            "set_canvas" => {
                let (w, h) = (super::need(a, "width")?.clamp(1.0, 30000.0) as u32, super::need(a, "height")?.clamp(1.0, 30000.0) as u32);
                let c = color4(str_arg(a, "color").or(Some("#FFFFFF")), [255, 255, 255, 255]);
                self.flatten_with(|img| effects::canvas(img, w, h, c))?;
                Ok(vec![text(format!("The canvas is now {w} x {h}."))])
            }
            "paste_from_clipboard" => {
                let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
                let img = cb.get_image().map_err(|_| "There's no picture on the clipboard.".to_string())?;
                let rgba = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()).ok_or("the clipboard picture couldn't be read")?;
                self.set(Doc::new(rgba), "From the clipboard")
            }
            "combine_images" => {
                let paths: Vec<String> = a.get("paths").and_then(Value::as_array).map(|v| v.iter().filter_map(|p| p.as_str().map(String::from)).collect()).unwrap_or_default();
                if paths.len() < 2 {
                    return Err("give two or more picture paths".into());
                }
                let mut imgs = Vec::new();
                for p in &paths {
                    imgs.push(image::open(p).map_err(|e| format!("{p}: {e}"))?.to_rgba8());
                }
                let vertical = a.get("vertical").and_then(Value::as_bool).unwrap_or(false);
                let out = effects::combine(&imgs, vertical, 16).ok_or("couldn't combine them")?;
                self.set(Doc::new(out), "Combined")
            }
            "make_guide" => {
                let title = str_arg(a, "title").unwrap_or("Guide").to_string();
                let path = PathBuf::from(str_arg(a, "path").ok_or("missing path")?);
                let list = a.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
                let current = a.get("include_current").and_then(Value::as_bool).unwrap_or(false);
                let total = (list.len() + current as usize + 1) as f64;
                let mut steps: Vec<(String, image::RgbaImage)> = Vec::new();
                for (i, s) in list.iter().enumerate() {
                    let p = s.get("path").and_then(Value::as_str).ok_or("each step needs a path")?;
                    let img = image::open(p).map_err(|e| format!("{p}: {e}"))?.to_rgba8();
                    let cap = s.get("caption").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("Step {}", i + 1));
                    steps.push((cap, img));
                    self.report(steps.len() as f64, Some(total), &format!("Read step {}", steps.len()));
                }
                if current {
                    let name = self.name.clone();
                    let d = self.doc()?.clone();
                    steps.push((name, render::compose(&d)));
                    self.report(steps.len() as f64, Some(total), "Added the current picture");
                }
                if steps.is_empty() {
                    return Err("give steps (or include_current)".into());
                }
                export::save_guide(&title, &steps, &path, self.settings().jpeg_quality)?;
                self.report(total, Some(total), "Saved");
                Ok(saved(&path, format!("Saved a {}-step guide to {}", steps.len(), path.display())))
            }
            "library_add" => {
                let s = self.settings();
                let title = str_arg(a, "title").map(String::from).unwrap_or_else(|| self.name.clone());
                let tags = str_list(a, "tags").unwrap_or_default();
                let d = self.doc()?.clone();
                let meta = library::Meta { title, kind: "AI".into(), tags, ..Default::default() };
                let p = library::add(Path::new(&s.library_folder), &render::compose(&d), meta)?;
                Ok(saved(&p, format!("Kept in the library: {}", p.display())))
            }
            "library_tag" | "library_delete" => {
                let s = self.settings();
                let want = PathBuf::from(str_arg(a, "path").ok_or("missing path")?);
                let mut item = library::scan(Path::new(&s.library_folder))
                    .into_iter()
                    .find(|i| same_file(&i.path, &want))
                    .ok_or("that picture isn't in Snagster's library (library_search lists them)")?;
                if name == "library_delete" {
                    library::delete(&item)?;
                    return Ok(vec![text(format!("Deleted {} from the library.", item.path.display()))]);
                }
                if let Some(t) = str_arg(a, "title") {
                    item.meta.title = t.to_string();
                }
                if let Some(t) = str_list(a, "tags") {
                    item.meta.tags = t;
                }
                for t in str_list(a, "add").unwrap_or_default() {
                    if !item.meta.tags.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                        item.meta.tags.push(t);
                    }
                }
                let gone = str_list(a, "remove").unwrap_or_default();
                item.meta.tags.retain(|x| !gone.iter().any(|g| g.eq_ignore_ascii_case(x)));
                item.save_meta()?;
                self.structured = Some(json!({"path": item.path, "title": item.meta.title, "tags": item.meta.tags}));
                Ok(vec![text(format!("\"{}\" has the tags: {}.", item.meta.title, if item.meta.tags.is_empty() { "none".into() } else { item.meta.tags.join(", ") })), link(&item.path)])
            }
            "list_recordings" => {
                let s = self.settings();
                let limit = num(a, "limit").unwrap_or(20.0) as usize;
                let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = std::fs::read_dir(&s.video_folder)
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| e.path().extension().is_some_and(|x| ["mp4", "wmv", "gif"].contains(&x.to_string_lossy().to_lowercase().as_str())))
                            .filter_map(|e| {
                                let m = e.metadata().ok()?;
                                Some((m.modified().ok()?, e.path(), m.len()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                files.sort_by(|a, b| b.0.cmp(&a.0));
                let list: Vec<Value> = files.into_iter().take(limit).map(|(_, p, n)| json!({"path": p, "kb": n / 1024})).collect();
                Ok(vec![json_text(&json!({"folder": s.video_folder, "recordings": list}))])
            }
            "video_info" => {
                let p = PathBuf::from(str_arg(a, "path").ok_or("missing path")?);
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif")) {
                    let frames = export::gif_frames(&p, 0)?;
                    let ms: u64 = frames.iter().map(|f| f.1 as u64).sum();
                    let (w, h) = frames.first().map(|f| f.0.dimensions()).unwrap_or((0, 0));
                    return Ok(vec![json_text(&json!({"frames": frames.len(), "seconds": ms as f64 / 1000.0, "width": w, "height": h}))]);
                }
                let pr = crate::video::probe(&p)?;
                Ok(vec![json_text(&json!({"frames": pr.frames, "seconds": pr.seconds, "width": pr.width, "height": pr.height}))])
            }
            "get_settings" => {
                let v = serde_json::to_value(self.settings()).map_err(|e| e.to_string())?;
                match str_arg(a, "key") {
                    Some(k) => Ok(vec![json_text(v.get(k).ok_or(format!("there's no setting {k}"))?)]),
                    None => Ok(vec![json_text(&v)]),
                }
            }
            "set_setting" => {
                let key = str_arg(a, "key").ok_or("missing key")?;
                let value = a.get("value").cloned().ok_or("missing value")?;
                let mut v = serde_json::to_value(self.settings()).map_err(|e| e.to_string())?;
                let slot = v.get_mut(key).ok_or(format!("there's no setting {key} (see get_settings)"))?;
                *slot = value;
                let s: crate::settings::Settings = serde_json::from_value(v).map_err(|e| format!("that value doesn't fit {key}: {e}"))?;
                self.save_settings(&s);
                Ok(vec![text(format!("Set {key}."))])
            }
            "list_languages" => {
                let list: Vec<Value> = crate::i18n::CODES.iter().map(|c| json!({"code": c, "name": crate::i18n::name_in(c, "en"), "native": crate::i18n::native_name(c)})).collect();
                Ok(vec![json_text(&list)])
            }
            "set_language" => {
                let code = str_arg(a, "code").ok_or("missing code")?;
                let mut s = self.settings();
                if code.eq_ignore_ascii_case("auto") {
                    s.language.clear();
                } else {
                    let c = crate::i18n::CODES.iter().find(|c| c.eq_ignore_ascii_case(code)).ok_or(format!("{code} isn't one of the 86 languages (see list_languages)"))?;
                    s.language = c.to_string();
                }
                self.save_settings(&s);
                let shown = crate::i18n::resolve(&s.language);
                Ok(vec![text(format!("The interface language is now {} ({}).", crate::i18n::name_in(shown, "en"), crate::i18n::native_name(shown)))])
            }
            "app_info" => {
                let s = self.settings();
                let lang = crate::i18n::resolve(&s.language);
                Ok(vec![json_text(&json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "system": std::env::consts::OS,
                    "architecture": std::env::consts::ARCH,
                    "features": {
                        "video_formats": crate::video::VideoFormat::video_names(),
                        "gif": true,
                        "studio": true,
                        "guides": ["pdf", "html", "docx", "png"],
                        "text_recognition": cfg!(windows),
                        "sound_recording": crate::audio::AudioSource::supported(),
                        "active_window_capture": cfg!(windows),
                    },
                    "language": {"code": lang, "name": crate::i18n::name_in(lang, "en"), "automatic": s.language.is_empty()},
                    "folders": {"pictures": s.save_folder, "library": s.library_folder, "recordings": s.video_folder},
                    "video_encoders": crate::video::encoders(),
                    "sound_recording": crate::audio::AudioSource::supported(),
                    "mcp": {"stdio": "snagster --mcp", "http": if s.mcp_http { format!("http://127.0.0.1:{}/mcp", s.mcp_port) } else { "off (Settings > AI apps, or run snagster mcp-http)".into() }},
                }))])
            }
            "check_for_update" => {
                let r = crate::update::latest()?;
                let current = env!("CARGO_PKG_VERSION");
                let newer = crate::update::is_newer(&r.version, current);
                Ok(vec![json_text(&json!({"current": current, "latest": r.version, "update_available": newer, "page": r.page}))])
            }
            "list_audio_devices" => Ok(vec![json_text(&json!({"sound_recording": crate::audio::AudioSource::supported(), "microphones": crate::audio::microphones()}))]),
            "open_in_snagster" => {
                let name = self.name.clone();
                let d = self.doc()?.clone();
                let dir = std::env::temp_dir().join("snagster-ai");
                let _ = std::fs::create_dir_all(&dir);
                let p = crate::settings::unique_path(&dir, "From AI", "snag");
                export::save_project(&d, &name, &p)?;
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                std::process::Command::new(exe).arg(&p).spawn().map_err(|e| e.to_string())?;
                Ok(vec![text("Opened in Snagster.")])
            }
            "list_presets" => {
                let s = self.settings();
                let list: Vec<Value> = s
                    .presets
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        json!({
                            "number": i,
                            "name": p.name,
                            "mode": p.mode.label(),
                            "delay_s": p.delay_s,
                            "effects": p.effects.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>(),
                            "hotkey": if p.combo.is_set() { p.combo.label() } else { String::new() }
                        })
                    })
                    .collect();
                Ok(vec![json_text(&list)])
            }
            "studio_scenes" => Ok(vec![json_text(&self.settings().studio)]),
            "studio_add_scene" => {
                let mut s = self.settings();
                let i = s.studio.add_scene(str_arg(a, "name"));
                let name = s.studio.scenes[i].name.clone();
                self.save_settings(&s);
                Ok(vec![text(format!("Added scene {i} ({name})."))])
            }
            "studio_remove_scene" | "studio_set_current" => {
                let mut s = self.settings();
                let i = index_arg(a, "scene")?;
                if name == "studio_set_current" {
                    s.studio.set_current(i)?;
                    let n = s.studio.scenes[i].name.clone();
                    self.save_settings(&s);
                    return Ok(vec![text(format!("Scene {i} ({n}) is now the current one."))]);
                }
                let gone = s.studio.remove_scene(i)?;
                self.save_settings(&s);
                Ok(vec![text(format!("Removed scene {i} ({}); the current scene is {}.", gone.name, s.studio.current))])
            }
            "studio_update_source" => {
                let mut s = self.settings();
                let si = scene_index(a, &s.studio)?;
                let i = index_arg(a, "index")?;
                let item = s.studio.scenes[si].items.get_mut(i).ok_or(format!("scene {si} has no source {i}"))?;
                let what = update_item(item, a)?;
                let n = item.name.clone();
                self.save_settings(&s);
                Ok(vec![text(format!("Source {i} ({n}) in scene {si}: changed {}.", what.join(", ")))])
            }
            "studio_add_source" => {
                let mut s = self.settings();
                let si = scene_index(a, &s.studio)?;
                let (cw, ch) = (s.studio.width as f32, s.studio.height as f32);
                let kind = match str_arg(a, "kind").unwrap_or("") {
                    "screen" => crate::studio::SourceKind::Display { index: num(a, "index").unwrap_or(0.0) as usize },
                    "window" => crate::studio::SourceKind::Window { title: str_arg(a, "title").ok_or("kind window needs a title")?.to_string() },
                    "area" => {
                        let r = rect_arg(a)?;
                        crate::studio::SourceKind::Area { x: r.min.x as i32, y: r.min.y as i32, w: r.width() as u32, h: r.height() as u32 }
                    }
                    "picture" => crate::studio::SourceKind::Image { path: str_arg(a, "path").ok_or("kind picture needs a path")?.to_string() },
                    "text" => crate::studio::SourceKind::Text {
                        text: str_arg(a, "text").ok_or("kind text needs text")?.to_string(),
                        size: num(a, "size").unwrap_or(48.0) as f32,
                        color: color4(str_arg(a, "color"), [255, 255, 255, 255]),
                    },
                    "color" => crate::studio::SourceKind::Color { color: color4(str_arg(a, "color"), [20, 20, 24, 255]) },
                    other => return Err(format!("unknown kind {other}")),
                };
                let item = crate::studio::Item {
                    name: str_arg(a, "name").map(String::from).unwrap_or_else(|| kind.label().to_string()),
                    kind,
                    x: num(a, "left").unwrap_or(0.0) as f32,
                    y: num(a, "top").unwrap_or(0.0) as f32,
                    w: num(a, "w").unwrap_or(cw as f64) as f32,
                    h: num(a, "h").unwrap_or(ch as f64) as f32,
                    visible: true,
                };
                s.studio.scenes[si].items.push(item);
                let n = s.studio.scenes[si].items.len() - 1;
                self.save_settings(&s);
                Ok(vec![text(format!("Added source {n} to scene {si}."))])
            }
            "studio_remove_source" => {
                let mut s = self.settings();
                let si = scene_index(a, &s.studio)?;
                let i = index_arg(a, "index")?;
                if i >= s.studio.scenes[si].items.len() {
                    return Err(format!("scene {si} has no source {i}"));
                }
                let gone = s.studio.scenes[si].items.remove(i);
                self.save_settings(&s);
                Ok(vec![text(format!("Removed {} from scene {si}.", gone.name))])
            }
            "studio_record" => {
                let s = self.settings();
                let si = scene_index(a, &s.studio)?;
                let secs = num(a, "seconds").ok_or("missing seconds")?.clamp(1.0, 3600.0) as u32;
                let mut opts = crate::studio::video_opts(&s);
                let mut note = "";
                if let Some(f) = str_arg(a, "format") {
                    (opts.format, note) = studio_format(f)?;
                }
                opts.path = match str_arg(a, "path") {
                    Some(p) => PathBuf::from(p),
                    None => crate::video::recording_path(&s.video_folder, "Studio", opts.format.ext()),
                };
                if let Some(f) = num(a, "fps") {
                    opts.fps = f.clamp(1.0, 120.0) as u32;
                }
                if let Some(x) = str_arg(a, "audio") {
                    opts.audio = crate::audio::AudioSource::parse(x).ok_or("audio is none, system, microphone or both")?;
                }
                if let Some(m) = str_arg(a, "microphone") {
                    opts.mic = m.to_string();
                }
                let d = self.ticking(Some(secs as f64), "Recording the Studio scene", || crate::studio::record_headless(&s.studio, Some(si), secs, opts, &self.stop));
                d.result.clone()?;
                Ok(saved(
                    &d.path,
                    format!(
                        "Saved {}: {:.1} s, {} frames, {} x {}. Encoder: {} ({}).{note}",
                        d.path.display(),
                        d.seconds,
                        d.frames,
                        d.width,
                        d.height,
                        d.encoder,
                        if d.hardware { "graphics chip" } else { "processor" }
                    ),
                ))
            }
            other => Err(format!("unknown tool {other} (tools/list shows them all)")),
        }
    }

    /// Capture the way a preset says: what, after how long, with which effects, then copy or
    /// save it.
    fn run_preset(&mut self, a: &Value) -> Outcome {
        let s = self.settings();
        let p = if num(a, "number").is_some() {
            let i = index_arg(a, "number")?;
            s.presets.get(i).cloned().ok_or(format!("there is no preset {i} (see list_presets)"))?
        } else if let Some(n) = str_arg(a, "name") {
            let low = n.to_lowercase();
            s.presets.iter().find(|p| p.name.to_lowercase() == low).or_else(|| s.presets.iter().find(|p| p.name.to_lowercase().contains(&low))).cloned().ok_or(format!("no preset is called {n} (see list_presets)"))?
        } else {
            return Err("give the preset's number or name (see list_presets)".into());
        };
        let (source, shape) = preset_plan(&p);
        let mut ca = CaptureArgs::parse(a)?;
        if num(a, "delay_s").is_none() {
            ca.delay_s = p.delay_s.min(60) as f64;
        }
        if ca.shape.is_none() {
            ca.shape = shape;
        }
        let area = || -> Result<(i32, i32, u32, u32), String> {
            let r = rect_arg(a).map_err(|e| format!("the preset \"{}\" captures an area: give x, y, width and height ({e})", p.name))?;
            Ok((r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32))
        };
        // Check the area before waiting.
        let rect = match source {
            PresetSource::Area | PresetSource::Scrolling => Some(area()?),
            PresetSource::Fixed(w, h) => {
                let (mx, my) = match (num(a, "x"), num(a, "y")) {
                    (Some(x), Some(y)) => (x as i32, y as i32),
                    _ => {
                        let m = crate::capture::monitor_under_cursor()?;
                        (m.0, m.1)
                    }
                };
                Some((mx, my, w, h))
            }
            _ => None,
        };
        self.wait(ca.delay_s)?;
        let g = match (&source, rect) {
            (PresetSource::Screen, _) => Grabbed::from_shot(capture::grab(&CaptureKind::Screen, (0, 0))?, None),
            (PresetSource::AllScreens, _) => Grabbed::from_shot(capture::grab(&CaptureKind::AllScreens, (0, 0))?, Some("All screens")),
            (PresetSource::ActiveWindow, _) => Grabbed::from_shot(capture::grab(&CaptureKind::ActiveWindow, (0, 0))?, None),
            (PresetSource::Scrolling, Some(r)) => {
                let img = self.ticking(None, "Scrolling and stitching", || capture::scroll_capture(r, false, s.scroll_max_steps.clamp(1, 200), s.scroll_wait_ms, self.stop.clone(), false))?;
                self.last_area = Some(r);
                Grabbed { image: img, origin: (r.0, r.1), cursor: None, title: p.name.clone(), app: capture::foreground_info().1 }
            }
            (_, Some(r)) => {
                let cursor = capture::cursor_pos();
                let img = capture::grab_rect(r.0, r.1, r.2, r.3)?;
                self.last_area = Some(r);
                Grabbed { image: img, origin: (r.0, r.1), cursor, title: p.name.clone(), app: capture::foreground_info().1 }
            }
            (_, None) => return Err("this preset can't run here".into()),
        };
        let kind = match source {
            PresetSource::Screen => "Screen",
            PresetSource::AllScreens => "All screens",
            PresetSource::ActiveWindow => "Window",
            _ => "Area",
        };
        let mut out = self.finish_capture(g, kind, &ca, &p.effects)?;
        let mut notes = vec![format!("Preset \"{}\".", p.name)];
        if p.copy {
            notes.push(self.copy_current().unwrap_or_else(|e| format!("Not copied: {e}")));
        }
        if p.save {
            match self.quick_save() {
                Ok(items) => {
                    notes.extend(items.iter().filter_map(|i| i.get("text").and_then(Value::as_str).map(String::from)));
                    out.extend(items.into_iter().filter(|i| i.get("type").and_then(Value::as_str) == Some("resource_link")));
                }
                Err(e) => notes.push(format!("Not saved: {e}")),
            }
        }
        if p.pin {
            notes.push("(Pinning to the screen happens only in the Snagster window.)".into());
        }
        if let Some(t) = out[0].get("text").and_then(Value::as_str) {
            let t = format!("{t} {}", notes.join(" "));
            out[0] = text(t);
        }
        Ok(out)
    }

    /// The current picture as a resource, when there is one.
    pub(super) fn current_png(&self) -> Option<(String, Value)> {
        let d = self.doc.as_ref()?;
        let p = preview(d, 4096).ok()?;
        Some((self.name.clone(), p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-mcp-more-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(dir: &Path, name: &str, w: u32, h: u32, c: [u8; 4]) -> PathBuf {
        let p = dir.join(name);
        image::RgbaImage::from_pixel(w, h, image::Rgba(c)).save(&p).unwrap();
        p
    }

    fn ws_with(img: image::RgbaImage) -> Workspace {
        let mut ws = Workspace::default();
        ws.set(Doc::new(img), "test").unwrap();
        ws
    }

    fn size(ws: &Workspace) -> (u32, u32) {
        ws.doc.as_ref().unwrap().image.dimensions()
    }

    #[test]
    fn colours_with_alpha_parse() {
        let d = [1, 2, 3, 4];
        assert_eq!(color4(None, d), d);
        assert_eq!(color4(Some("#102030"), d), [0x10, 0x20, 0x30, 255]);
        assert_eq!(color4(Some("102030"), d), [0x10, 0x20, 0x30, 255], "the # is optional");
        assert_eq!(color4(Some(" #10203080 "), d), [0x10, 0x20, 0x30, 0x80]);
        assert_eq!(color4(Some("#zz2030"), d), [255, 0x20, 0x30, 255], "a bad pair reads as ff");
        assert_eq!(color4(Some("white"), d), [255, 255, 255, 255]);
        assert_eq!(color4(Some("blue"), d), [0, 144, 255, 255]);
        assert_eq!(color4(Some("#abc"), d), [229, 72, 77, 255], "anything else is the default red");
    }

    #[test]
    fn scenes_are_picked_by_number_or_the_current_one() {
        let mut st = crate::studio::StudioSettings::default();
        st.scenes.push(crate::studio::Scene { name: "Two".into(), items: Vec::new() });
        st.current = 1;
        assert_eq!(scene_index(&json!({}), &st), Ok(1));
        assert_eq!(scene_index(&json!({"scene": 0}), &st), Ok(0));
        assert_eq!(scene_index(&json!({"scene": "x"}), &st), Ok(1), "not a number: the current one");
        assert!(scene_index(&json!({"scene": 2}), &st).unwrap_err().contains("no scene 2"));
        st.current = 5;
        assert!(scene_index(&json!({}), &st).is_err(), "a stale current scene is refused, not indexed");
    }

    /// Bug: scene -1 became scene 0 (`as usize` saturates).
    #[test]
    fn a_negative_scene_number_is_refused() {
        let st = crate::studio::StudioSettings::default();
        assert!(scene_index(&json!({"scene": -1}), &st).is_err());
    }

    #[test]
    fn small_helpers() {
        let a = json!({"s": "x", "n": 1});
        assert_eq!(str_arg(&a, "s"), Some("x"));
        assert_eq!(str_arg(&a, "n"), None);
        assert_eq!(str_arg(&a, "missing"), None);
        let t = json_text(&json!({"a": [1, 2]}));
        assert_eq!(t["type"], "text");
        let back: Value = serde_json::from_str(t["text"].as_str().unwrap()).unwrap();
        assert_eq!(back, json!({"a": [1, 2]}));
    }

    #[test]
    fn the_extra_tools_have_their_own_names() {
        let base: Vec<&str> = super::super::base_tools().iter().map(|t| t.name).collect();
        for t in tools() {
            assert!(!base.contains(&t.name), "{} is defined twice", t.name);
            assert!(!t.description.is_empty(), "{}", t.name);
        }
    }

    #[test]
    fn trim_cuts_flat_edges() {
        let mut img = image::RgbaImage::from_pixel(50, 40, image::Rgba([255, 255, 255, 255]));
        for y in 10..20 {
            for x in 5..25 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut ws = ws_with(img);
        let t = ws.call("trim", &json!({})).unwrap();
        let (w, h) = size(&ws);
        assert!(w < 50 && h < 40 && w >= 20 && h >= 10, "{w} x {h}");
        assert_eq!(t[0]["text"], format!("Trimmed to {w} x {h}."));
        ws.call("undo", &json!({})).unwrap();
        assert_eq!(size(&ws), (50, 40));
    }

    #[test]
    fn the_canvas_grows_around_the_picture_in_the_given_colour() {
        let mut ws = ws_with(image::RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255])));
        assert!(ws.call("set_canvas", &json!({"width": 30})).is_err());
        ws.call("set_canvas", &json!({"width": 30, "height": 20, "color": "#FF000080"})).unwrap();
        let d = ws.doc.as_ref().unwrap();
        assert_eq!(d.image.dimensions(), (30, 20));
        assert_eq!(d.image.get_pixel(0, 0).0, [255, 0, 0, 0x80]);
        assert_eq!(d.image.get_pixel(15, 10).0, [0, 0, 0, 255], "the picture is in the middle");
        // Sizes are kept between 1 and 30000.
        ws.call("set_canvas", &json!({"width": -5, "height": 0})).unwrap();
        assert_eq!(size(&ws), (1, 1));
        ws.call("set_canvas", &json!({"width": 4, "height": 4})).unwrap();
        assert_eq!(ws.doc.as_ref().unwrap().image.get_pixel(0, 0).0, [255, 255, 255, 255], "default white");
    }

    #[test]
    fn redo_comes_back_after_undo() {
        let mut ws = ws_with(image::RgbaImage::new(20, 20));
        ws.call("set_canvas", &json!({"width": 40, "height": 40})).unwrap();
        ws.call("undo", &json!({})).unwrap();
        assert_eq!(ws.call("redo", &json!({})).unwrap()[0]["text"], "Redone.");
        assert_eq!(size(&ws), (40, 40));
        assert!(ws.call("redo", &json!({})).is_err());
        ws.call("undo", &json!({})).unwrap();
        assert_eq!(size(&ws), (20, 20));
    }

    #[test]
    fn combining_needs_two_pictures_that_open() {
        let dir = temp_dir("combine");
        let a = png(&dir, "a.png", 30, 20, [255, 0, 0, 255]);
        let b = png(&dir, "b.png", 10, 40, [0, 255, 0, 255]);
        let mut ws = Workspace::default();
        assert_eq!(ws.call("combine_images", &json!({})).unwrap_err(), "give two or more picture paths");
        assert!(ws.call("combine_images", &json!({"paths": [a]})).is_err());
        let e = ws.call("combine_images", &json!({"paths": [a, dir.join("nope.png")]})).unwrap_err();
        assert!(e.contains("nope.png"), "{e}");
        ws.call("combine_images", &json!({"paths": [a, b]})).unwrap();
        let (w, h) = size(&ws);
        assert!(w >= 40 && h >= 40, "side by side: {w} x {h}");
        ws.call("combine_images", &json!({"paths": [a, b], "vertical": true})).unwrap();
        let (w, h) = size(&ws);
        assert!(w >= 30 && h >= 60, "top to bottom: {w} x {h}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guides_check_steps_and_format_and_save() {
        let dir = temp_dir("guide");
        let a = png(&dir, "a.png", 40, 30, [10, 20, 30, 255]);
        let mut ws = Workspace::default();
        assert_eq!(ws.call("make_guide", &json!({})).unwrap_err(), "missing path");
        assert_eq!(ws.call("make_guide", &json!({"path": dir.join("g.pdf")})).unwrap_err(), "give steps (or include_current)");
        assert!(ws.call("make_guide", &json!({"path": dir.join("g.pdf"), "include_current": true})).unwrap_err().contains("no current picture"));
        assert_eq!(ws.call("make_guide", &json!({"path": dir.join("g.pdf"), "steps": [{"caption": "x"}]})).unwrap_err(), "each step needs a path");
        let e = ws.call("make_guide", &json!({"path": dir.join("g.txt"), "steps": [{"path": a}]})).unwrap_err();
        assert!(e.contains(".pdf, .html"), "{e}");
        let html = dir.join("sub").join("g.html");
        let r = ws.call("make_guide", &json!({"title": "My <guide>", "path": html, "steps": [{"path": a, "caption": "First"}, {"path": a}]})).unwrap();
        assert!(r[0]["text"].as_str().unwrap().starts_with("Saved a 2-step guide"));
        let page = std::fs::read_to_string(&html).unwrap();
        assert!(page.contains("First") && page.contains("Step 2"), "captions, with a default for the second");
        assert!(page.contains("My &lt;guide&gt;"), "the title is escaped");
        ws.set(Doc::new(image::RgbaImage::new(16, 16)), "Now").unwrap();
        let pdf = dir.join("g.pdf");
        ws.call("make_guide", &json!({"path": pdf, "include_current": true})).unwrap();
        assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn video_info_needs_a_readable_file() {
        let dir = temp_dir("video-info");
        let mut ws = Workspace::default();
        assert_eq!(ws.call("video_info", &json!({})).unwrap_err(), "missing path");
        assert!(ws.call("video_info", &json!({"path": dir.join("none.gif")})).is_err());
        assert!(ws.call("video_info", &json!({"path": dir.join("none.mp4")})).is_err());
        let junk = dir.join("junk.mp4");
        std::fs::write(&junk, b"not a video").unwrap();
        assert!(ws.call("video_info", &json!({"path": junk})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// These refuse before anything is saved, so the person's settings are left alone.
    #[test]
    fn settings_tools_check_their_arguments() {
        let mut ws = Workspace::default();
        assert_eq!(ws.call("set_setting", &json!({})).unwrap_err(), "missing key");
        assert_eq!(ws.call("set_setting", &json!({"key": "video_fps"})).unwrap_err(), "missing value");
        assert!(ws.call("set_setting", &json!({"key": "no_such_setting_xyz", "value": 1})).unwrap_err().contains("no setting no_such_setting_xyz"));
        assert!(ws.call("set_setting", &json!({"key": "video_fps", "value": "lots"})).unwrap_err().contains("doesn't fit video_fps"));
        assert!(ws.call("get_settings", &json!({"key": "no_such_setting_xyz"})).unwrap_err().contains("no setting"));
        assert_eq!(ws.call("set_language", &json!({})).unwrap_err(), "missing code");
        assert!(ws.call("set_language", &json!({"code": "xx-nope"})).unwrap_err().contains("isn't one of"));
    }

    #[test]
    fn languages_are_listed_with_names() {
        let mut ws = Workspace::default();
        let r = ws.call("list_languages", &json!({})).unwrap();
        let list: Value = serde_json::from_str(r[0]["text"].as_str().unwrap()).unwrap();
        let list = list.as_array().unwrap();
        assert_eq!(list.len(), crate::i18n::CODES.len());
        assert!(list.iter().all(|l| l["code"].is_string() && l["name"].is_string() && l["native"].is_string()));
        assert!(list.iter().any(|l| l["code"] == "en"));
    }

    /// These refuse before anything is saved, so the person's Studio setup is left alone.
    #[test]
    fn studio_tools_check_their_arguments() {
        let mut ws = Workspace::default();
        let big = json!({"scene": 9999, "kind": "color"});
        assert!(ws.call("studio_add_source", &big).unwrap_err().contains("no scene 9999"));
        assert!(ws.call("studio_add_source", &json!({"scene": 0, "kind": "hologram"})).unwrap_err().contains("unknown kind hologram"));
        assert!(ws.call("studio_add_source", &json!({"scene": 0})).unwrap_err().contains("unknown kind"));
        assert_eq!(ws.call("studio_add_source", &json!({"scene": 0, "kind": "window"})).unwrap_err(), "kind window needs a title");
        assert_eq!(ws.call("studio_add_source", &json!({"scene": 0, "kind": "picture"})).unwrap_err(), "kind picture needs a path");
        assert_eq!(ws.call("studio_add_source", &json!({"scene": 0, "kind": "text"})).unwrap_err(), "kind text needs text");
        assert!(ws.call("studio_add_source", &json!({"scene": 0, "kind": "area", "x": 0, "y": 0, "width": 0, "height": 5})).is_err());
        assert!(ws.call("studio_remove_source", &json!({"scene": 0})).unwrap_err().contains("index"));
        assert!(ws.call("studio_remove_source", &json!({"scene": 0, "index": 1_000_000})).unwrap_err().contains("has no source"));
        assert!(ws.call("studio_remove_source", &json!({"scene": 0, "index": -1})).unwrap_err().contains("0 or more"));
        assert!(ws.call("studio_remove_source", &json!({"scene": -1, "index": 0})).unwrap_err().contains("0 or more"));
        assert_eq!(ws.call("studio_record", &json!({"scene": 0})).unwrap_err(), "missing seconds");
        assert!(ws.call("studio_record", &json!({"scene": 0, "seconds": 1, "format": "avi"})).unwrap_err().contains("format is one of"));
        assert!(ws.call("studio_record", &json!({"scene": 0, "seconds": 1, "audio": "loud"})).unwrap_err().contains("audio is none"));
        assert!(ws.call("studio_update_source", &json!({"scene": 0, "index": 1_000_000, "name": "x"})).unwrap_err().contains("has no source"));
        assert!(ws.call("studio_remove_scene", &json!({})).unwrap_err().contains("scene"));
        assert!(ws.call("studio_set_current", &json!({"scene": 1_000_000})).unwrap_err().contains("no scene"));
    }

    /// Studio records video: a GIF format records MP4 (as the Studio window does) and says so.
    #[test]
    fn studio_formats_follow_the_studio_window() {
        use crate::video::VideoFormat;
        let (f, note) = studio_format("gif").unwrap();
        assert_eq!(f, VideoFormat::Mp4);
        assert!(note.contains("GIF became MP4"));
        assert_eq!(studio_format("mp4").unwrap(), (VideoFormat::Mp4, ""));
        assert!(studio_format("avi").unwrap_err().contains("format is one of mp4"));
        assert_eq!(studio_format("wmv").is_ok(), cfg!(windows));
        let list = super::super::tool_list();
        let t = list["tools"].as_array().unwrap().iter().find(|t| t["name"] == "studio_record").unwrap().clone();
        assert!(t["inputSchema"]["properties"]["format"]["enum"].as_array().unwrap().contains(&json!("gif")));
        assert!(t["description"].as_str().unwrap().contains("gif records MP4"));
    }

    fn fixed(tag: &str) -> (PathBuf, Workspace) {
        let dir = temp_dir(tag);
        let s = crate::settings::Settings {
            library_folder: dir.join("lib").to_string_lossy().to_string(),
            save_folder: dir.join("saved").to_string_lossy().to_string(),
            video_folder: dir.join("videos").to_string_lossy().to_string(),
            ..Default::default()
        };
        (dir, Workspace::with_settings(s))
    }

    #[test]
    fn studio_scenes_are_added_picked_changed_and_removed() {
        let (dir, mut ws) = fixed("studio-scenes");
        assert_eq!(ws.call("studio_add_scene", &json!({"name": "Talk"})).unwrap()[0]["text"], "Added scene 1 (Talk).");
        ws.call("studio_set_current", &json!({"scene": 1})).unwrap();
        assert_eq!(ws.settings().studio.current, 1);
        ws.call("studio_add_source", &json!({"kind": "text", "text": "Hello", "size": 30})).unwrap();
        ws.call("studio_add_source", &json!({"kind": "color", "color": "#102030"})).unwrap();
        let t = ws.call("studio_update_source", &json!({"index": 0, "text": "Bye", "left": 10, "top": 20, "w": 300, "visible": false})).unwrap();
        assert!(t[0]["text"].as_str().unwrap().contains("place, size, shown, look"), "{t:?}");
        let item = ws.settings().studio.scenes[1].items[0].clone();
        assert_eq!((item.x, item.y, item.w, item.visible), (10.0, 20.0, 300.0, false));
        assert!(matches!(item.kind, crate::studio::SourceKind::Text { ref text, size, .. } if text == "Bye" && size == 30.0));
        assert!(ws.call("studio_update_source", &json!({"index": 1, "title": "x"})).unwrap_err().contains("a color source has no title"));
        assert!(ws.call("studio_update_source", &json!({"index": 1, "text": "x"})).unwrap_err().contains("has no text"));
        ws.call("studio_update_source", &json!({"index": 1, "color": "#FF000080"})).unwrap();
        assert!(matches!(ws.settings().studio.scenes[1].items[1].kind, crate::studio::SourceKind::Color { color: [255, 0, 0, 0x80] }));
        assert!(ws.call("studio_update_source", &json!({"index": 1})).unwrap_err().contains("nothing to change"));
        assert!(ws.call("studio_update_source", &json!({"index": 1, "w": 0})).unwrap_err().contains("1 or more"));
        // Removing the current scene moves to the one before; the last scene stays.
        assert!(ws.call("studio_remove_scene", &json!({"scene": 1})).unwrap()[0]["text"].as_str().unwrap().contains("current scene is 0"));
        assert!(ws.call("studio_remove_scene", &json!({"scene": 0})).unwrap_err().contains("at least one"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn studio_sources_change_what_they_show() {
        use crate::studio::{Item, SourceKind};
        let item = |kind| Item { name: "s".into(), kind, x: 0.0, y: 0.0, w: 10.0, h: 10.0, visible: true };
        let mut d = item(SourceKind::Display { index: 0 });
        update_item(&mut d, &json!({"screen": 2, "name": "Two"})).unwrap();
        assert_eq!((d.name.as_str(), &d.kind), ("Two", &SourceKind::Display { index: 2 }));
        assert!(update_item(&mut d, &json!({"screen": -1})).unwrap_err().contains("0 or more"));
        let mut w = item(SourceKind::Window { title: "a".into() });
        update_item(&mut w, &json!({"title": "Editor"})).unwrap();
        assert_eq!(w.kind, SourceKind::Window { title: "Editor".into() });
        let mut a = item(SourceKind::Area { x: 0, y: 0, w: 1, h: 1 });
        update_item(&mut a, &json!({"x": 5, "y": 6, "width": 70, "height": 80})).unwrap();
        assert_eq!(a.kind, SourceKind::Area { x: 5, y: 6, w: 70, h: 80 });
        assert!(update_item(&mut a, &json!({"x": 5})).is_err(), "an area needs all four numbers");
        let mut p = item(SourceKind::Image { path: "a.png".into() });
        update_item(&mut p, &json!({"path": "b.png"})).unwrap();
        assert_eq!(p.kind, SourceKind::Image { path: "b.png".into() });
        assert!(update_item(&mut p, &json!({"screen": 1})).unwrap_err().contains("a picture source has no screen"));
    }

    #[test]
    fn presets_capture_the_way_they_say() {
        let p = |mode, w, h| Preset { mode, width: w, height: h, ..Default::default() };
        assert_eq!(preset_plan(&p(PresetMode::Screen, 0, 0)), (PresetSource::Screen, None));
        assert_eq!(preset_plan(&p(PresetMode::AllScreens, 0, 0)), (PresetSource::AllScreens, None));
        assert_eq!(preset_plan(&p(PresetMode::Window, 0, 0)), (PresetSource::ActiveWindow, None));
        assert_eq!(preset_plan(&p(PresetMode::Area, 0, 0)), (PresetSource::Area, None));
        assert_eq!(preset_plan(&p(PresetMode::Circle, 0, 0)), (PresetSource::Area, Some(CutShape::Ellipse)));
        assert_eq!(preset_plan(&p(PresetMode::Star, 0, 0)), (PresetSource::Area, Some(CutShape::Star)));
        assert_eq!(preset_plan(&p(PresetMode::Heart, 0, 0)), (PresetSource::Area, Some(CutShape::Heart)));
        assert_eq!(preset_plan(&p(PresetMode::FixedSize, 640, 2)), (PresetSource::Fixed(640, 8), None), "at least 8 pixels");
        assert_eq!(preset_plan(&p(PresetMode::Scrolling, 0, 0)), (PresetSource::Scrolling, None));
        // Every mode has a plan.
        for m in PresetMode::ALL {
            let _ = preset_plan(&p(m, 10, 10));
        }
    }

    #[test]
    fn run_preset_checks_its_arguments_before_waiting() {
        let (dir, mut ws) = fixed("presets");
        assert!(ws.call("run_preset", &json!({})).unwrap_err().contains("number or name"));
        assert!(ws.call("run_preset", &json!({"number": 99})).unwrap_err().contains("no preset 99"));
        assert!(ws.call("run_preset", &json!({"number": -1})).unwrap_err().contains("0 or more"));
        assert!(ws.call("run_preset", &json!({"name": "nothing like it"})).unwrap_err().contains("no preset is called"));
        // The default presets: an area with border and shadow, a 1280 x 720 frame, a round avatar.
        let e = ws.call("run_preset", &json!({"name": "avatar"})).unwrap_err();
        assert!(e.contains("Round avatar") && e.contains("x, y, width and height"), "{e}");
        let e = ws.call("run_preset", &json!({"number": 0, "x": 0, "y": 0, "width": 0, "height": 5})).unwrap_err();
        assert!(e.contains("above 0"), "{e}");
        assert!(ws.call("run_preset", &json!({"number": 0, "x": 0, "y": 0, "width": 5, "height": 5, "shape": "blob"})).unwrap_err().contains("shape"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_pictures_are_tagged_and_deleted_only_inside_the_library() {
        let (dir, mut ws) = fixed("library");
        ws.set(Doc::new(image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))), "pic").unwrap();
        let r = ws.call("library_add", &json!({"title": "First", "tags": ["a", " ", "b"]})).unwrap();
        assert_eq!(r[1]["type"], "resource_link");
        let lib = dir.join("lib");
        let path = library::scan(&lib)[0].path.clone();
        assert_eq!(library::scan(&lib)[0].meta.tags, ["a", "b"], "blank tags are dropped");
        let r = ws.call("library_tag", &json!({"path": path, "add": ["c", "A"], "remove": ["b"], "title": "Renamed"})).unwrap();
        assert!(r[0]["text"].as_str().unwrap().contains("a, c"), "{r:?}");
        assert_eq!(ws.structured.as_ref().unwrap()["tags"], json!(["a", "c"]), "adding a tag it has (any case) does nothing");
        let item = &library::scan(&lib)[0];
        assert_eq!((item.meta.title.as_str(), item.meta.tags.clone()), ("Renamed", vec!["a".to_string(), "c".to_string()]));
        ws.call("library_tag", &json!({"path": path, "tags": []})).unwrap();
        assert!(library::scan(&lib)[0].meta.tags.is_empty());
        // Pictures elsewhere are refused, even ones that exist.
        let outside = png(&dir, "outside.png", 4, 4, [0, 0, 0, 255]);
        for tool in ["library_tag", "library_delete"] {
            assert!(ws.call(tool, &json!({"path": outside})).unwrap_err().contains("isn't in Snagster's library"), "{tool}");
            assert!(ws.call(tool, &json!({})).unwrap_err().contains("missing path"), "{tool}");
        }
        assert!(outside.exists());
        ws.call("library_delete", &json!({"path": path})).unwrap();
        assert!(!path.exists() && !path.with_extension("json").exists());
        assert!(library::scan(&lib).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_are_changed_and_read_back() {
        let (dir, mut ws) = fixed("settings");
        ws.call("set_setting", &json!({"key": "video_fps", "value": 48})).unwrap();
        let v: Value = serde_json::from_str(ws.call("get_settings", &json!({"key": "video_fps"})).unwrap()[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v, 48);
        ws.call("set_language", &json!({"code": "DE"})).unwrap();
        assert_eq!(ws.settings().language, "de");
        ws.call("set_language", &json!({"code": "auto"})).unwrap();
        assert!(ws.settings().language.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guides_report_progress_step_by_step() {
        let (dir, mut ws) = fixed("guide-progress");
        let a = png(&dir, "a.png", 10, 10, [9, 9, 9, 255]);
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let g = got.clone();
        ws.progress = Some(std::sync::Arc::new(move |d: f64, t: Option<f64>, m: &str| g.lock().unwrap().push((d, t, m.to_string()))));
        ws.set(Doc::new(image::RgbaImage::new(5, 5)), "Now").unwrap();
        let r = ws.call("make_guide", &json!({"path": dir.join("g.pdf"), "steps": [{"path": a}, {"path": a}], "include_current": true})).unwrap();
        assert_eq!(r[1]["type"], "resource_link");
        assert_eq!(r[1]["mimeType"], "application/pdf");
        let got = got.lock().unwrap();
        let done: Vec<f64> = got.iter().map(|p| p.0).collect();
        assert_eq!(done, [1.0, 2.0, 3.0, 4.0]);
        assert!(got.iter().all(|p| p.1 == Some(4.0)));
        assert_eq!(got[3].2, "Saved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_info_lists_what_this_system_can_do() {
        let (dir, mut ws) = fixed("app-info");
        let r = ws.call("app_info", &json!({})).unwrap();
        let v: Value = serde_json::from_str(r[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["features"]["text_recognition"], cfg!(windows));
        assert_eq!(v["features"]["video_formats"], json!(crate::video::VideoFormat::video_names()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_current_picture_resource_is_a_png() {
        let ws = Workspace::default();
        assert!(ws.current_png().is_none());
        let ws = ws_with(image::RgbaImage::new(5, 6));
        let (name, img) = ws.current_png().unwrap();
        assert_eq!(name, "test");
        assert_eq!(img["mimeType"], "image/png");
    }

    #[test]
    fn library_add_needs_a_picture() {
        let mut ws = Workspace::default();
        assert!(ws.call("library_add", &json!({"title": "x"})).unwrap_err().contains("no current picture"));
    }
}
