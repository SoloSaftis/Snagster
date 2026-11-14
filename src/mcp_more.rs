//! The rest of Snagster's commands as MCP tools: more ways to capture, editing, guides,
//! the library, settings, languages, updates, sound devices, Studio scenes and recordings.

use super::{Outcome, Tool, Workspace, index_arg, num, obj, preview, rect_arg, text};
use crate::capture::{self, CaptureKind};
use crate::doc::Doc;
use crate::{effects, export, library, ocr, render};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn tools() -> Vec<Tool> {
    let n = |d: &str| json!({"type": "number", "description": d});
    let s = |d: &str| json!({"type": "string", "description": d});
    let b = |d: &str| json!({"type": "boolean", "description": d});
    let rect = || json!({"x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number"}, "height": {"type": "number"}});
    let with = |mut base: Value, extra: Value| {
        if let (Some(m), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                m.insert(k.clone(), v.clone());
            }
        }
        base
    };
    vec![
        Tool { name: "capture_active_window", description: "Capture the window in front as the current picture.", schema: obj(json!({}), &[]), edits: true },
        Tool {
            name: "capture_scrolling",
            description: "Scroll a desktop area (a web page, a document) and stitch it into one long picture, as the current picture. Keep the mouse still while it runs.",
            schema: obj(with(rect(), json!({"horizontal": b("Scroll sideways instead (panorama)."), "max_steps": n("Most scroll steps, default 40.")})), &["x", "y", "width", "height"]),
            edits: true,
        },
        Tool { name: "pick_color", description: "The colour of one desktop pixel, as #RRGGBB.", schema: obj(json!({"x": {"type": "number"}, "y": {"type": "number"}}), &["x", "y"]), edits: false },
        Tool { name: "read_screen_text", description: "Read the text in a desktop rectangle (Windows text recognition), without changing the current picture.", schema: obj(rect(), &["x", "y", "width", "height"]), edits: false },
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
            description: "Make a step-by-step guide from pictures with captions: a PDF, a web page (html), a Word document (docx) or one tall picture (png).",
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
        Tool { name: "list_presets", description: "The capture presets the person set up.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "studio_scenes", description: "Studio mode's scenes and their sources (screens, windows, areas, pictures, text, colours) with positions.", schema: obj(json!({}), &[]), edits: false },
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
            description: "Record a Studio scene (all its sources composed together) to a video file for some seconds, with sound if asked.",
            schema: obj(
                json!({
                    "seconds": n("1-3600."),
                    "scene": n("Scene number (default: the current one)."),
                    "fps": n("1-120, default Studio's setting."),
                    "format": {"type": "string", "enum": ["mp4", "hevc", "wmv"]},
                    "audio": {"type": "string", "enum": ["none", "system", "microphone", "both"]},
                    "path": s("Where to save it; default the Videos/Snagster folder.")
                }),
                &["seconds"],
            ),
            edits: false,
        },
    ]
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

fn settings() -> crate::settings::Settings {
    crate::settings::Settings::load(false)
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
                let shot = capture::grab(&CaptureKind::ActiveWindow, (0, 0))?;
                let t = shot.title.clone();
                self.set(Doc::new(shot.image), &t)
            }
            "capture_scrolling" => {
                let r = rect_arg(a)?;
                let horizontal = a.get("horizontal").and_then(Value::as_bool).unwrap_or(false);
                let steps = num(a, "max_steps").unwrap_or(40.0).clamp(1.0, 200.0) as u32;
                self.stop.store(false, std::sync::atomic::Ordering::Relaxed);
                let img = capture::scroll_capture((r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32), horizontal, steps, 350, self.stop.clone(), false)?;
                self.set(Doc::new(img), if horizontal { "Panorama" } else { "Scrolling capture" })
            }
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
                let mut steps: Vec<(String, image::RgbaImage)> = Vec::new();
                if let Some(list) = a.get("steps").and_then(Value::as_array) {
                    for (i, s) in list.iter().enumerate() {
                        let p = s.get("path").and_then(Value::as_str).ok_or("each step needs a path")?;
                        let img = image::open(p).map_err(|e| format!("{p}: {e}"))?.to_rgba8();
                        let cap = s.get("caption").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("Step {}", i + 1));
                        steps.push((cap, img));
                    }
                }
                if a.get("include_current").and_then(Value::as_bool).unwrap_or(false) {
                    let name = self.name.clone();
                    let d = self.doc()?.clone();
                    steps.push((name, render::compose(&d)));
                }
                if steps.is_empty() {
                    return Err("give steps (or include_current)".into());
                }
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                match ext.as_str() {
                    "pdf" => {
                        let pages: Vec<(Option<String>, image::RgbaImage)> = steps.iter().map(|(c, i)| (Some(c.clone()), i.clone())).collect();
                        std::fs::write(&path, export::pdf(&pages, true, 90)?).map_err(|e| e.to_string())?;
                    }
                    "html" | "htm" => std::fs::write(&path, export::html_guide(&title, &steps)?).map_err(|e| e.to_string())?,
                    "docx" => export::docx(&title, &steps, &path)?,
                    "png" | "jpg" | "jpeg" => {
                        let page = effects::guide_page(&title, &steps, 1200).ok_or("couldn't lay out the page")?;
                        export::save_image(&page, &path, 90)?;
                    }
                    _ => return Err("the path must end in .pdf, .html, .docx or .png".into()),
                }
                Ok(vec![text(format!("Saved a {}-step guide to {}", steps.len(), path.display()))])
            }
            "library_add" => {
                let s = settings();
                let title = str_arg(a, "title").map(String::from).unwrap_or_else(|| self.name.clone());
                let tags: Vec<String> = a.get("tags").and_then(Value::as_array).map(|v| v.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default();
                let d = self.doc()?.clone();
                let meta = library::Meta { title, kind: "AI".into(), tags, ..Default::default() };
                let p = library::add(Path::new(&s.library_folder), &render::compose(&d), meta)?;
                Ok(vec![text(format!("Kept in the library: {}", p.display()))])
            }
            "list_recordings" => {
                let s = settings();
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
                let v = serde_json::to_value(settings()).map_err(|e| e.to_string())?;
                match str_arg(a, "key") {
                    Some(k) => Ok(vec![json_text(v.get(k).ok_or(format!("there's no setting {k}"))?)]),
                    None => Ok(vec![json_text(&v)]),
                }
            }
            "set_setting" => {
                let key = str_arg(a, "key").ok_or("missing key")?;
                let value = a.get("value").cloned().ok_or("missing value")?;
                let mut v = serde_json::to_value(settings()).map_err(|e| e.to_string())?;
                let slot = v.get_mut(key).ok_or(format!("there's no setting {key} (see get_settings)"))?;
                *slot = value;
                let s: crate::settings::Settings = serde_json::from_value(v).map_err(|e| format!("that value doesn't fit {key}: {e}"))?;
                s.save();
                Ok(vec![text(format!("Set {key}."))])
            }
            "list_languages" => {
                let list: Vec<Value> = crate::i18n::CODES.iter().map(|c| json!({"code": c, "name": crate::i18n::name_in(c, "en"), "native": crate::i18n::native_name(c)})).collect();
                Ok(vec![json_text(&list)])
            }
            "set_language" => {
                let code = str_arg(a, "code").ok_or("missing code")?;
                let mut s = settings();
                if code.eq_ignore_ascii_case("auto") {
                    s.language.clear();
                } else {
                    let c = crate::i18n::CODES.iter().find(|c| c.eq_ignore_ascii_case(code)).ok_or(format!("{code} isn't one of the 86 languages (see list_languages)"))?;
                    s.language = c.to_string();
                }
                s.save();
                let shown = crate::i18n::resolve(&s.language);
                Ok(vec![text(format!("The interface language is now {} ({}).", crate::i18n::name_in(shown, "en"), crate::i18n::native_name(shown)))])
            }
            "app_info" => {
                let s = settings();
                let lang = crate::i18n::resolve(&s.language);
                Ok(vec![json_text(&json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "system": std::env::consts::OS,
                    "architecture": std::env::consts::ARCH,
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
                let s = settings();
                let list: Vec<Value> = s.presets.iter().enumerate().map(|(i, p)| json!({"number": i, "name": p.name, "hotkey": if p.combo.is_set() { p.combo.label() } else { String::new() }})).collect();
                Ok(vec![json_text(&list)])
            }
            "studio_scenes" => Ok(vec![json_text(&settings().studio)]),
            "studio_add_source" => {
                let mut s = settings();
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
                s.save();
                Ok(vec![text(format!("Added source {n} to scene {si}."))])
            }
            "studio_remove_source" => {
                let mut s = settings();
                let si = scene_index(a, &s.studio)?;
                let i = index_arg(a, "index")?;
                if i >= s.studio.scenes[si].items.len() {
                    return Err(format!("scene {si} has no source {i}"));
                }
                let gone = s.studio.scenes[si].items.remove(i);
                s.save();
                Ok(vec![text(format!("Removed {} from scene {si}.", gone.name))])
            }
            "studio_record" => {
                let s = settings();
                let si = scene_index(a, &s.studio)?;
                let secs = num(a, "seconds").ok_or("missing seconds")?.clamp(1.0, 3600.0) as u32;
                let format = match str_arg(a, "format") {
                    Some(f) => crate::video::VideoFormat::parse(f).filter(|f| *f != crate::video::VideoFormat::Gif).ok_or("format is mp4, hevc or wmv")?,
                    None => crate::video::VideoFormat::Mp4,
                };
                let path = match str_arg(a, "path") {
                    Some(p) => PathBuf::from(p),
                    None => {
                        let name = chrono::Local::now().format("Studio_%Y-%m-%d_%H-%M-%S").to_string();
                        crate::settings::unique_path(&PathBuf::from(&s.video_folder), &name, format.ext())
                    }
                };
                let fps = num(a, "fps").map(|f| f.clamp(1.0, 120.0) as u32).unwrap_or(s.studio.fps);
                let mut opts = crate::video::VideoOpts::screen((0, 0, s.studio.width, s.studio.height), fps, format, path);
                opts.quality = s.video_quality;
                opts.hardware = s.video_hardware;
                opts.audio = match str_arg(a, "audio") {
                    Some(x) => crate::audio::AudioSource::parse(x).ok_or("audio is none, system, microphone or both")?,
                    None => crate::audio::AudioSource::None,
                };
                opts.mic = s.audio_mic.clone();
                let d = crate::studio::record_headless(&s.studio, Some(si), secs, opts);
                d.result.clone()?;
                Ok(vec![text(format!(
                    "Saved {}: {:.1} s, {} frames, {} x {}. Encoder: {} ({}).",
                    d.path.display(),
                    d.seconds,
                    d.frames,
                    d.width,
                    d.height,
                    d.encoder,
                    if d.hardware { "graphics chip" } else { "processor" }
                ))])
            }
            other => Err(format!("unknown tool {other} (tools/list shows them all)")),
        }
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
        assert_eq!(ws.call("studio_record", &json!({"scene": 0, "seconds": 1, "format": "gif"})).unwrap_err(), "format is mp4, hevc or wmv");
        assert!(ws.call("studio_record", &json!({"scene": 0, "seconds": 1, "audio": "loud"})).unwrap_err().contains("audio is none"));
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
