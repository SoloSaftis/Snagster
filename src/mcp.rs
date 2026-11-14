//! Snagster for AI apps: an MCP server (JSON-RPC 2.0 over stdin/stdout, one message per line)
//! and the same tools as command-line commands. Start it with `snagster --mcp`.
//!
//! The tools work on one "current picture" kept in memory: capture or open one, add marks,
//! blur, crop, apply effects, read its text, then save or copy it.

use crate::capture::{self, CaptureKind};
use crate::doc::{self, Doc, Shape};
use crate::{effects, export, library, ocr, render};
use base64::Engine as _;
use eframe::egui::{Color32, Rect, pos2, vec2};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const INSTRUCTIONS: &str = "Snagster captures the screen and marks up pictures. Typical use: capture_screen \
(or capture_region / capture_window / open_image) to get a current picture, then add_mark, blur_area, crop, \
apply_effect or read_text, then view_picture to look at it and save or copy_to_clipboard. record_video records the screen to an MP4 file at 1-120 frames per second. Coordinates are \
picture pixels from the top-left corner. Colours are #RRGGBB or a name (red, orange, yellow, green, blue, \
purple, black, white).";

/// The picture the tools work on.
#[derive(Default)]
pub struct Workspace {
    doc: Option<Doc>,
    name: String,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    /// Set to stop a running recording (an AI app cancelled the call).
    pub stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// A tool's answer: MCP content items, or an error message.
pub type Outcome = Result<Vec<Value>, String>;

#[path = "mcp_more.rs"]
mod more;

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    edits: bool,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn effect_names() -> Vec<String> {
    effects::Effect::ALL.iter().map(|e| format!("{e:?}")).collect()
}

fn tools() -> Vec<Tool> {
    let mut all = base_tools();
    all.extend(more::tools());
    all
}

fn base_tools() -> Vec<Tool> {
    let n = |d: &str| json!({"type": "number", "description": d});
    let s = |d: &str| json!({"type": "string", "description": d});
    let rect = || json!({"x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number"}, "height": {"type": "number"}});
    let with = |mut base: Value, extra: Value| {
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    };
    vec![
        Tool { name: "list_monitors", description: "The screens: id, name, position and size in desktop pixels.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "list_windows", description: "Visible windows: id, title, program, position and size.", schema: obj(json!({}), &[]), edits: false },
        Tool {
            name: "capture_screen",
            description: "Capture a screen (the one under the mouse, a monitor by id, or all screens) as the current picture. Returns a preview.",
            schema: obj(json!({"monitor_id": n("A monitor id from list_monitors."), "all_screens": {"type": "boolean"}}), &[]),
            edits: true,
        },
        Tool {
            name: "capture_region",
            description: "Capture a desktop rectangle (desktop pixels) as the current picture.",
            schema: obj(rect(), &["x", "y", "width", "height"]),
            edits: true,
        },
        Tool {
            name: "capture_window",
            description: "Capture one window, by id or by part of its title, as the current picture.",
            schema: obj(json!({"id": n("A window id from list_windows."), "title": s("Part of the window title.")}), &[]),
            edits: true,
        },
        Tool { name: "open_image", description: "Open a picture file (PNG, JPEG, BMP, GIF, WebP, TIFF) or a .snag project as the current picture.", schema: obj(json!({"path": s("Full path.")}), &["path"]), edits: true },
        Tool { name: "picture_info", description: "Size of the current picture and its marks.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "view_picture", description: "The current picture with its marks, as an image (scaled to at most max_size pixels).", schema: obj(json!({"max_size": n("Longest side, default 1024.")}), &[]), edits: false },
        Tool {
            name: "add_mark",
            description: "Add a mark. kind: arrow, line, box, oval, highlight, text, callout, step, stamp, pointer, spotlight, magnify. Arrows/lines go from (x, y) to (x2, y2); boxes, ovals, highlight and spotlight cover (x, y)-(x2, y2); text, callout, step, stamp, pointer and magnify are placed at (x, y) (a callout points at (x2, y2)).",
            schema: obj(
                json!({
                    "kind": {"type": "string", "enum": ["arrow", "line", "box", "oval", "highlight", "text", "callout", "step", "stamp", "pointer", "spotlight", "magnify"]},
                    "x": {"type": "number"}, "y": {"type": "number"}, "x2": {"type": "number"}, "y2": {"type": "number"},
                    "text": s("Text for text and callout; the emoji for stamp."),
                    "color": s("#RRGGBB or a colour name. Default red."),
                    "width": n("Line width in pixels, default 6."),
                    "size": n("Text size, stamp size or magnify radius."),
                    "fill": {"type": "boolean", "description": "Fill boxes and ovals."},
                    "shadow": {"type": "boolean"},
                    "number": n("Number for a step; default the next one.")
                }),
                &["kind", "x", "y"],
            ),
            edits: true,
        },
        Tool { name: "list_marks", description: "The marks on the current picture, numbered from 0.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "remove_mark", description: "Remove a mark by its number from list_marks.", schema: obj(json!({"index": n("Mark number.")}), &["index"]), edits: true },
        Tool {
            name: "blur_area",
            description: "Hide part of the picture: pixelate (default), blur or a solid box.",
            schema: obj(with(rect(), json!({"mode": {"type": "string", "enum": ["pixelate", "blur", "solid"]}, "color": s("For solid.")})), &["x", "y", "width", "height"]),
            edits: true,
        },
        Tool { name: "crop", description: "Keep only a rectangle of the picture.", schema: obj(rect(), &["x", "y", "width", "height"]), edits: true },
        Tool { name: "resize", description: "Resize the picture to an exact size (marks are flattened first).", schema: obj(json!({"width": {"type": "number"}, "height": {"type": "number"}}), &["width", "height"]), edits: true },
        Tool {
            name: "apply_effect",
            description: "Apply a picture effect (marks are flattened first).",
            schema: obj(json!({"effect": {"type": "string", "enum": effect_names()}}), &["effect"]),
            edits: true,
        },
        Tool { name: "read_text", description: "Read the text in the current picture (Windows text recognition): every line with its box.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "smart_redact", description: "Find emails, phone numbers, card numbers, addresses, IP and web addresses in the picture and blur them.", schema: obj(json!({}), &[]), edits: true },
        Tool { name: "undo", description: "Undo the last change to the current picture.", schema: obj(json!({}), &[]), edits: true },
        Tool {
            name: "save",
            description: "Save the current picture. The extension picks the format: png, jpg, bmp, gif, webp, tiff, pdf, svg, docx, or snag (keeps marks editable).",
            schema: obj(json!({"path": s("Full path with extension.")}), &["path"]),
            edits: false,
        },
        Tool { name: "copy_to_clipboard", description: "Copy the current picture (with marks) to the clipboard.", schema: obj(json!({}), &[]), edits: false },
        Tool {
            name: "record_gif",
            description: "Record a desktop rectangle as an animated GIF for some seconds and save it.",
            schema: obj(with(rect(), json!({"seconds": n("1-120, default 5."), "fps": n("1-30, default 10."), "path": s("Where to save the .gif.")})), &["x", "y", "width", "height", "path"]),
            edits: false,
        },
        Tool {
            name: "record_video",
            description: "Record the screen (or a desktop rectangle, or one monitor) to a video file for some seconds, at 1-120 frames per second. Uses the graphics chip's encoder (Intel, AMD or NVIDIA) when there is one. Returns the file, its length, frames and the encoder used.",
            schema: obj(
                json!({
                    "seconds": n("1-600."),
                    "fps": n("Frames per second, 1-120, default 30."),
                    "format": {"type": "string", "enum": ["mp4", "hevc", "wmv"], "description": "mp4 (H.264, default), hevc (MP4 with H.265) or wmv."},
                    "quality": n("1 small file, 2 good (default), 3 high, 4 best."),
                    "hardware": {"type": "boolean", "description": "Use the graphics chip's encoder (default true)."},
                    "cursor": {"type": "boolean", "description": "Draw the mouse pointer (default true)."},
                    "monitor_id": n("Record this monitor (from list_monitors)."),
                    "x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number"}, "height": {"type": "number"},
                    "path": s("Where to save it; default the Recordings folder in Snagster's library.")
                }),
                &["seconds"],
            ),
            edits: false,
        },
        Tool { name: "list_video_encoders", description: "The video encoders this computer has (graphics-chip and software) for H.264, HEVC and WMV.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "library_search", description: "Search Snagster's capture library by title, program, tag or date.", schema: obj(json!({"query": s("Words to find; empty lists everything."), "limit": n("Default 20.")}), &[]), edits: false },
    ]
}

/// The `tools/list` result, with a title and safety hints for every tool.
pub fn tool_list() -> Value {
    // Tools that change something outside the current picture (files, clipboard, settings, screen).
    const SIDE: [&str; 17] = [
        "save", "copy_to_clipboard", "record_gif", "record_video", "make_guide", "library_add", "set_setting", "set_language",
        "open_in_snagster", "studio_add_source", "studio_remove_source", "studio_record", "capture_scrolling", "paste_from_clipboard",
        "capture_screen", "capture_region", "capture_window",
    ];
    const DESTRUCTIVE: [&str; 3] = ["remove_mark", "clear_marks", "studio_remove_source"];
    let tools: Vec<Value> = tools()
        .into_iter()
        .map(|t| {
            let read_only = !t.edits && !SIDE.contains(&t.name);
            let mut title: String = t.name.replace('_', " ");
            if let Some(c) = title.get(0..1) {
                title = c.to_uppercase() + &title[1..];
            }
            json!({
                "name": t.name,
                "title": title,
                "description": t.description,
                "inputSchema": t.schema,
                "annotations": {
                    "title": title,
                    "readOnlyHint": read_only,
                    "destructiveHint": DESTRUCTIVE.contains(&t.name),
                    "idempotentHint": read_only,
                    "openWorldHint": t.name == "check_for_update",
                },
            })
        })
        .collect();
    json!({ "tools": tools })
}

pub fn tool_names() -> Vec<(&'static str, &'static str)> {
    tools().into_iter().map(|t| (t.name, t.description)).collect()
}

fn text(s: impl Into<String>) -> Value {
    json!({"type": "text", "text": s.into()})
}

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64)
}

fn need(a: &Value, k: &str) -> Result<f64, String> {
    num(a, k).ok_or_else(|| format!("missing number \"{k}\""))
}

fn rect_arg(a: &Value) -> Result<Rect, String> {
    let (x, y, w, h) = (need(a, "x")?, need(a, "y")?, need(a, "width")?, need(a, "height")?);
    if w <= 0.0 || h <= 0.0 {
        return Err("width and height must be above 0".into());
    }
    Ok(Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32)))
}

pub fn parse_color(s: Option<&str>) -> Color32 {
    let s = s.unwrap_or("red").trim().to_lowercase();
    if let Some(hex) = s.strip_prefix('#')
        && hex.len() == 6
        && let Ok(v) = u32::from_str_radix(hex, 16)
    {
        return Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
    }
    let named = [
        ("red", [229, 72, 77]),
        ("orange", [247, 107, 21]),
        ("yellow", [255, 197, 61]),
        ("green", [48, 164, 108]),
        ("blue", [0, 144, 255]),
        ("purple", [142, 78, 198]),
        ("black", [28, 28, 32]),
        ("white", [255, 255, 255]),
    ];
    named.iter().find(|(n, _)| *n == s).map(|(_, c)| Color32::from_rgb(c[0], c[1], c[2])).unwrap_or(Color32::from_rgb(229, 72, 77))
}

/// A PNG of the picture with its marks, at most `max` pixels on the longest side.
fn preview(d: &Doc, max: u32) -> Result<Value, String> {
    let flat = render::compose(d);
    let (w, h) = flat.dimensions();
    let img = if w.max(h) > max { image::imageops::thumbnail(&flat, (w * max / w.max(h)).max(1), (h * max / w.max(h)).max(1)) } else { flat };
    let png = export::png_bytes(&img)?;
    Ok(json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(png), "mimeType": "image/png"}))
}

impl Workspace {
    fn doc(&mut self) -> Result<&mut Doc, String> {
        self.doc.as_mut().ok_or_else(|| "There is no current picture yet: capture_screen or open_image first.".to_string())
    }

    fn set(&mut self, d: Doc, name: &str) -> Outcome {
        let (w, h) = d.image.dimensions();
        self.doc = Some(d);
        self.name = name.to_string();
        self.undo.clear();
        let p = preview(self.doc.as_ref().unwrap(), 1024)?;
        Ok(vec![text(format!("Current picture: {name}, {w} x {h} pixels.")), p])
    }

    fn change(&mut self, f: impl FnOnce(&Doc) -> Result<Doc, String>) -> Result<(), String> {
        let cur = self.doc()?.clone();
        let next = f(&cur)?;
        self.undo.push(cur);
        if self.undo.len() > 50 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.doc = Some(next);
        Ok(())
    }

    fn flatten_with(&mut self, f: impl FnOnce(&image::RgbaImage) -> image::RgbaImage) -> Result<(), String> {
        self.change(|d| Ok(Doc::new(f(&render::compose(d)))))
    }

    /// Run one tool.
    pub fn call(&mut self, name: &str, a: &Value) -> Outcome {
        match name {
            "list_monitors" => {
                let list: Vec<Value> = capture::monitors()
                    .into_iter()
                    .map(|m| json!({"id": m.id, "name": m.name, "x": m.x, "y": m.y, "width": m.width, "height": m.height, "primary": m.primary}))
                    .collect();
                Ok(vec![text(serde_json::to_string_pretty(&list).unwrap_or_default())])
            }
            "list_windows" => {
                let list: Vec<Value> = capture::list_windows()
                    .into_iter()
                    .map(|w| json!({"id": w.id, "title": w.label, "program": w.app, "x": w.x, "y": w.y, "width": w.width, "height": w.height}))
                    .collect();
                Ok(vec![text(serde_json::to_string_pretty(&list).unwrap_or_default())])
            }
            "capture_screen" => {
                let kind = if a.get("all_screens").and_then(Value::as_bool).unwrap_or(false) {
                    CaptureKind::AllScreens
                } else if let Some(id) = num(a, "monitor_id") {
                    CaptureKind::Monitor(id as u32)
                } else {
                    CaptureKind::Screen
                };
                let shot = capture::grab(&kind, (0, 0))?;
                let title = shot.title.clone();
                self.set(Doc::new(shot.image), &title)
            }
            "capture_region" => {
                let r = rect_arg(a)?;
                let img = capture::grab_rect(r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32)?;
                self.set(Doc::new(img), "Area")
            }
            "capture_window" => {
                let wins = capture::list_windows();
                let found = if let Some(id) = num(a, "id") {
                    wins.into_iter().find(|w| w.id == id as u32)
                } else if let Some(t) = a.get("title").and_then(Value::as_str) {
                    let t = t.to_lowercase();
                    wins.into_iter().find(|w| w.label.to_lowercase().contains(&t))
                } else {
                    return Err("give id or title".into());
                };
                let w = found.ok_or("no such window (see list_windows)")?;
                let shot = capture::grab(&CaptureKind::Window(w.id, w.label.clone()), (0, 0))?;
                self.set(Doc::new(shot.image), &w.label)
            }
            "open_image" => {
                let p = std::path::PathBuf::from(a.get("path").and_then(Value::as_str).ok_or("missing path")?);
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("snag")) {
                    let (name, d) = export::load_project(&p)?;
                    return self.set(d, &name);
                }
                let img = image::open(&p).map_err(|e| e.to_string())?.to_rgba8();
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                self.set(Doc::new(img), &name)
            }
            "picture_info" => {
                let name = self.name.clone();
                let d = self.doc()?;
                let (w, h) = d.image.dimensions();
                Ok(vec![text(format!("{name}: {w} x {h} pixels, {} marks.", d.shapes.len()))])
            }
            "view_picture" => {
                let max = num(a, "max_size").unwrap_or(1024.0).clamp(64.0, 4096.0) as u32;
                let d = self.doc()?.clone();
                Ok(vec![preview(&d, max)?])
            }
            "add_mark" => {
                let shape = make_mark(a, self.doc()?)?;
                let label = describe(&shape);
                self.change(|d| {
                    let mut d = d.clone();
                    d.shapes.push(shape);
                    Ok(d)
                })?;
                let n = self.doc()?.shapes.len() - 1;
                Ok(vec![text(format!("Added mark {n}: {label}."))])
            }
            "list_marks" => {
                let d = self.doc()?;
                let lines: Vec<String> = d.shapes.iter().enumerate().map(|(i, s)| format!("{i}: {}", describe(s))).collect();
                Ok(vec![text(if lines.is_empty() { "No marks.".into() } else { lines.join("\n") })])
            }
            "remove_mark" => {
                let i = need(a, "index")? as usize;
                self.change(|d| {
                    if i >= d.shapes.len() {
                        return Err("no mark with that number".into());
                    }
                    let mut d = d.clone();
                    d.shapes.remove(i);
                    Ok(d)
                })?;
                Ok(vec![text(format!("Removed mark {i}."))])
            }
            "blur_area" => {
                let r = rect_arg(a)?;
                let mode = match a.get("mode").and_then(Value::as_str).unwrap_or("pixelate") {
                    "blur" => crate::settings::BlurMode::Blur,
                    "solid" => crate::settings::BlurMode::Solid,
                    _ => crate::settings::BlurMode::Pixelate,
                };
                let color = parse_color(a.get("color").and_then(Value::as_str).or(Some("black")));
                self.change(|d| d.redact(r, mode, 12, color).ok_or("that area is outside the picture".into()))?;
                Ok(vec![text("Hidden.")])
            }
            "crop" => {
                let r = rect_arg(a)?;
                self.change(|d| d.crop(r).ok_or("that area is outside the picture".into()))?;
                let (w, h) = self.doc()?.image.dimensions();
                Ok(vec![text(format!("Cropped to {w} x {h}."))])
            }
            "resize" => {
                let (w, h) = (need(a, "width")? as u32, need(a, "height")? as u32);
                self.flatten_with(|i| effects::resize_exact(i, w, h))?;
                Ok(vec![text(format!("Resized to {w} x {h}."))])
            }
            "apply_effect" => {
                let want = a.get("effect").and_then(Value::as_str).ok_or("missing effect")?;
                let e = effects::Effect::ALL
                    .into_iter()
                    .find(|e| format!("{e:?}").eq_ignore_ascii_case(want) || e.label().eq_ignore_ascii_case(want))
                    .ok_or_else(|| format!("unknown effect; use one of {}", effect_names().join(", ")))?;
                let s = crate::settings::Settings::load(false);
                let opts = effects::EffectOptions {
                    resize_percent: s.resize_percent,
                    border_width: s.border_width,
                    border_color: s.border_color,
                    shadow_size: s.shadow_size,
                    rounded_radius: s.rounded_radius,
                    watermark_text: s.watermark_text.clone(),
                    tilt_percent: s.tilt_percent,
                    brightness: s.brightness,
                    contrast: s.contrast,
                    saturation: s.saturation,
                    info_text: format!("{}   {}", chrono::Local::now().format("%Y-%m-%d %H:%M"), self.name),
                };
                self.flatten_with(|i| effects::apply(e, i, &opts))?;
                Ok(vec![text(format!("{} applied.", e.label()))])
            }
            "read_text" => {
                let d = self.doc()?.clone();
                let lines = ocr::recognize(&render::compose(&d))?;
                let list: Vec<Value> = lines
                    .iter()
                    .map(|l| {
                        let r = l.rect();
                        json!({"text": l.text, "x": r.min.x.round(), "y": r.min.y.round(), "width": r.width().round(), "height": r.height().round()})
                    })
                    .collect();
                Ok(vec![text(ocr::plain_text(&lines)), text(serde_json::to_string(&list).unwrap_or_default())])
            }
            "smart_redact" => {
                let d = self.doc()?.clone();
                let lines = ocr::recognize(&render::compose(&d))?;
                let found = ocr::find_sensitive(
                    &lines,
                    ocr::RedactKinds { emails: true, phones: true, cards: true, addresses: true, ips: true, urls: false },
                );
                if found.is_empty() {
                    return Ok(vec![text("Nothing sensitive was found.")]);
                }
                self.change(|d| {
                    let mut d = d.clone();
                    for (r, _) in &found {
                        if let Some(n) = d.blur(*r, 12) {
                            d = n;
                        }
                    }
                    Ok(d)
                })?;
                let kinds: Vec<&str> = found.iter().map(|f| f.1).collect();
                Ok(vec![text(format!("Blurred {} items: {}.", found.len(), kinds.join(", ")))])
            }
            "undo" => {
                let prev = self.undo.pop().ok_or("nothing to undo")?;
                if let Some(cur) = self.doc.replace(prev) {
                    self.redo.push(cur);
                }
                Ok(vec![text("Undone.")])
            }
            "save" => {
                let p = std::path::PathBuf::from(a.get("path").and_then(Value::as_str).ok_or("missing path")?);
                let name = self.name.clone();
                let d = self.doc()?.clone();
                let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                match ext.as_str() {
                    "snag" => export::save_project(&d, &name, &p)?,
                    "docx" => export::docx(&name, &[(name.clone(), render::compose(&d))], &p)?,
                    _ => export::save_image(&render::compose(&d), &p, 90)?,
                }
                Ok(vec![text(format!("Saved {}", p.display()))])
            }
            "copy_to_clipboard" => {
                let flat = render::compose(self.doc()?);
                let (w, h) = flat.dimensions();
                let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: flat.into_raw().into() };
                arboard::Clipboard::new().and_then(|mut c| c.set_image(data)).map_err(|e| e.to_string())?;
                Ok(vec![text(format!("Copied the {w} x {h} picture."))])
            }
            "list_video_encoders" => {
                let list = crate::video::encoders();
                Ok(vec![text(serde_json::to_string_pretty(&list).unwrap_or_default())])
            }
            "record_video" => {
                let secs = num(a, "seconds").ok_or("missing seconds")?.clamp(1.0, 600.0) as u32;
                let format = match a.get("format").and_then(Value::as_str) {
                    Some(f) => crate::video::VideoFormat::parse(f).filter(|f| *f != crate::video::VideoFormat::Gif).ok_or("format is mp4, hevc or wmv (use record_gif for GIFs)")?,
                    None => crate::video::VideoFormat::Mp4,
                };
                let rect = if a.get("width").is_some() {
                    let r = rect_arg(a)?;
                    (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32)
                } else {
                    let mons = capture::monitors();
                    let m = match num(a, "monitor_id") {
                        Some(id) => mons.iter().find(|m| m.id == id as u32).ok_or("no such monitor (see list_monitors)")?,
                        None => {
                            let at = capture::cursor_pos().unwrap_or((0, 0));
                            mons.iter()
                                .find(|m| at.0 >= m.x && at.1 >= m.y && at.0 < m.x + m.width as i32 && at.1 < m.y + m.height as i32)
                                .or(mons.first())
                                .ok_or("no screen found")?
                        }
                    };
                    (m.x, m.y, m.width, m.height)
                };
                let path = match a.get("path").and_then(Value::as_str) {
                    Some(p) => std::path::PathBuf::from(p),
                    None => {
                        let dir = crate::settings::Settings::load(false).video_folder;
                        let name = chrono::Local::now().format("Recording_%Y-%m-%d_%H-%M-%S").to_string();
                        crate::settings::unique_path(&std::path::PathBuf::from(dir), &name, format.ext())
                    }
                };
                let opts = crate::video::VideoOpts {
                    rect,
                    fps: num(a, "fps").unwrap_or(30.0).clamp(1.0, 120.0) as u32,
                    format,
                    quality: num(a, "quality").unwrap_or(2.0).clamp(1.0, 4.0) as u8,
                    hardware: a.get("hardware").and_then(Value::as_bool).unwrap_or(true),
                    cursor: a.get("cursor").and_then(Value::as_bool).unwrap_or(true),
                    max_s: secs,
                    path,
                    audio: match a.get("audio").and_then(Value::as_str) {
                        Some(s) => crate::audio::AudioSource::parse(s).ok_or("audio is none, system, microphone or both")?,
                        None => crate::audio::AudioSource::None,
                    },
                    mic: a.get("microphone").and_then(Value::as_str).unwrap_or_default().to_string(),
                    frames: None,
                };
                self.stop.store(false, std::sync::atomic::Ordering::Relaxed);
                let pause = std::sync::atomic::AtomicBool::new(false);
                let d = crate::video::record(&opts, &self.stop, &pause);
                d.result.clone()?;
                let size = std::fs::metadata(&d.path).map(|m| m.len()).unwrap_or(0);
                Ok(vec![text(format!(
                    "Saved {}: {:.1} s, {} frames at {} fps, {} x {}, {} KB. Encoder: {} ({}); screen copied by {}.",
                    d.path.display(),
                    d.seconds,
                    d.frames,
                    opts.fps,
                    d.width,
                    d.height,
                    size / 1024,
                    d.encoder,
                    if d.hardware { "graphics chip" } else { "processor" },
                    d.capture
                ))])
            }
            "record_gif" => {
                let r = rect_arg(a)?;
                let secs = num(a, "seconds").unwrap_or(5.0).clamp(1.0, 120.0) as u32;
                let fps = num(a, "fps").unwrap_or(10.0).clamp(1.0, 30.0) as u32;
                let path = std::path::PathBuf::from(a.get("path").and_then(Value::as_str).ok_or("missing path")?);
                self.stop.store(false, std::sync::atomic::Ordering::Relaxed);
                let stop = self.stop.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                capture::spawn_gif_recorder(
                    (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32),
                    fps,
                    secs,
                    1280,
                    true,
                    path,
                    stop,
                    eframe::egui::Context::default(),
                    tx,
                );
                let done = rx.recv_timeout(std::time::Duration::from_secs(secs as u64 + 60)).map_err(|e| e.to_string())?;
                done.result?;
                Ok(vec![text(format!("Recorded {:.1} s ({} frames) to {}", done.seconds, done.frames, done.path.display()))])
            }
            "library_search" => {
                let s = crate::settings::Settings::load(false);
                let q = a.get("query").and_then(Value::as_str).unwrap_or("");
                let limit = num(a, "limit").unwrap_or(20.0) as usize;
                let items: Vec<Value> = library::scan(std::path::Path::new(&s.library_folder))
                    .into_iter()
                    .filter(|i| i.matches(q))
                    .take(limit)
                    .map(|i| json!({"path": i.path, "title": i.meta.title, "program": i.meta.app, "created": i.meta.created, "tags": i.meta.tags}))
                    .collect();
                Ok(vec![text(serde_json::to_string_pretty(&items).unwrap_or_default())])
            }
            other => self.call_more(other, a),
        }
    }
}

fn describe(s: &Shape) -> String {
    let b = s.bounds();
    let kind = match s {
        Shape::Arrow { .. } => "arrow",
        Shape::Line { .. } => "line",
        Shape::Rect { .. } => "box",
        Shape::Ellipse { .. } => "oval",
        Shape::Pen { .. } => "pen",
        Shape::Highlight { .. } => "highlight",
        Shape::Text { text, .. } => return format!("text \"{text}\" at {:.0},{:.0}", b.min.x, b.min.y),
        Shape::Callout { text, .. } => return format!("callout \"{text}\" at {:.0},{:.0}", b.min.x, b.min.y),
        Shape::Step { n, .. } => return format!("step {n} at {:.0},{:.0}", b.center().x, b.center().y),
        Shape::Stamp { emoji, .. } => return format!("stamp {emoji} at {:.0},{:.0}", b.center().x, b.center().y),
        Shape::Magnify { .. } => "magnifier",
        Shape::Pointer { .. } => "pointer",
        Shape::Spotlight { .. } => "spotlight",
        Shape::Image { .. } => "picture piece",
    };
    format!("{kind} at {:.0},{:.0} to {:.0},{:.0}", b.min.x, b.min.y, b.max.x, b.max.y)
}

fn make_mark(a: &Value, d: &Doc) -> Result<Shape, String> {
    let kind = a.get("kind").and_then(Value::as_str).ok_or("missing kind")?;
    let p1 = pos2(need(a, "x")? as f32, need(a, "y")? as f32);
    let p2 = match (num(a, "x2"), num(a, "y2")) {
        (Some(x), Some(y)) => Some(pos2(x as f32, y as f32)),
        _ => None,
    };
    let need2 = || p2.ok_or_else(|| format!("{kind} needs x2 and y2"));
    let color = parse_color(a.get("color").and_then(Value::as_str));
    let width = num(a, "width").unwrap_or(6.0) as f32;
    let size = num(a, "size").map(|s| s as f32);
    let fill = a.get("fill").and_then(Value::as_bool).unwrap_or(false);
    let shadow = a.get("shadow").and_then(Value::as_bool).unwrap_or(false);
    let txt = a.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    Ok(match kind {
        "arrow" => Shape::Arrow { a: p1, b: need2()?, color, width, shadow, heads: 1 },
        "line" => Shape::Line { a: p1, b: need2()?, color, width, shadow },
        "box" => Shape::Rect { rect: Rect::from_two_pos(p1, need2()?), color, width, fill, radius: 0.0, shadow },
        "oval" => Shape::Ellipse { rect: Rect::from_two_pos(p1, need2()?), color, width, fill, shadow },
        "highlight" => {
            let b = need2()?;
            Shape::Highlight { points: vec![pos2(p1.x, (p1.y + b.y) / 2.0), pos2(b.x, (p1.y + b.y) / 2.0)], color, width: ((b.y - p1.y).abs() / doc::HIGHLIGHT_WIDTH_FACTOR).max(1.0) }
        }
        "spotlight" => Shape::Spotlight { rect: Rect::from_two_pos(p1, need2()?) },
        "text" => Shape::Text {
            pos: p1,
            text: txt,
            color,
            size: size.unwrap_or(doc::text_size(width)),
            bg: fill,
            shadow,
            font: String::new(),
            bold: false,
            italic: false,
            outline: true,
            align: 0,
        },
        "callout" => Shape::Callout {
            pos: p1,
            tail: p2.unwrap_or(p1 + vec2(-40.0, 60.0)),
            text: txt,
            color,
            size: size.unwrap_or(doc::text_size(width)),
            shadow,
            font: String::new(),
            bold: false,
            italic: false,
            shape: Default::default(),
            outline: false,
            align: 0,
        },
        "step" => Shape::Step {
            center: p1,
            n: num(a, "number").map(|n| n as u32).unwrap_or_else(|| d.next_step(1)),
            color,
            radius: size.unwrap_or(doc::step_radius(width)),
            shadow,
            style: Default::default(),
        },
        "stamp" => Shape::Stamp {
            center: p1,
            emoji: if txt.is_empty() { "\u{2705}".into() } else { txt },
            size: size.unwrap_or(doc::text_size(width) * doc::STAMP_FACTOR),
            color,
        },
        "pointer" => Shape::Pointer { pos: p1, size: size.unwrap_or(doc::pointer_size(width)) },
        "magnify" => Shape::Magnify { center: p1, radius: size.unwrap_or(80.0), zoom: 2.0 },
        other => return Err(format!("unknown kind {other}")),
    })
}

fn result(id: &Value, result: Value) -> Option<String> {
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
}

fn error(id: &Value, code: i64, message: &str) -> Option<String> {
    Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string())
}

/// One AI app's connection: its current picture, and a flag that stops a running recording when
/// the app cancels the call (reachable without waiting for the running tool).
pub struct Session {
    ws: std::sync::Mutex<Workspace>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Default for Session {
    fn default() -> Self {
        let ws = Workspace::default();
        let stop = ws.stop.clone();
        Session { ws: std::sync::Mutex::new(ws), stop }
    }
}

const PROMPTS: [(&str, &str, &str, &str); 4] = [
    (
        "annotate_screenshot",
        "Capture the screen and point out something on it with arrows, boxes and text.",
        "what",
        "Capture my screen with capture_screen, find {what} on it, and mark it clearly with add_mark (an arrow or a box and a short text label). Then show me the result with view_picture.",
    ),
    (
        "redact_screenshot",
        "Capture a window and hide private details before sharing.",
        "window",
        "Capture the window titled {window} with capture_window, run smart_redact, then look at it with view_picture and blur_area anything private that is still visible. Finally copy_to_clipboard.",
    ),
    (
        "record_demo",
        "Record a short screen video with sound.",
        "seconds",
        "Record my screen with record_video for {seconds} seconds at 30 fps with audio set to system, then tell me where the file was saved.",
    ),
    (
        "step_guide",
        "Make a step-by-step guide from pictures.",
        "topic",
        "Help me make a step-by-step guide about {topic}: capture each step as I go, then use make_guide to save a PDF with a caption for every step.",
    ),
];

fn prompts_list() -> Value {
    let list: Vec<Value> = PROMPTS
        .iter()
        .map(|(name, desc, arg, _)| json!({"name": name, "description": desc, "arguments": [{"name": arg, "required": false}]}))
        .collect();
    json!({ "prompts": list })
}

fn prompts_get(params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
    let (_, desc, arg, body) = PROMPTS.iter().find(|p| p.0 == name).ok_or((-32602, format!("no prompt {name}")))?;
    let value = params.pointer(&format!("/arguments/{arg}")).and_then(Value::as_str).unwrap_or(match *arg {
        "seconds" => "10",
        "what" => "the most important button",
        "window" => "the window in front",
        _ => "this task",
    });
    Ok(json!({"description": desc, "messages": [{"role": "user", "content": {"type": "text", "text": body.replace(&format!("{{{arg}}}"), value)}}]}))
}

impl Session {
    /// Handle one JSON-RPC message; the reply line, or None for notifications and replies.
    pub fn handle(&self, line: &str) -> Option<String> {
        let message: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return error(&Value::Null, -32700, "Parse error"),
        };
        if message.is_array() {
            return error(&Value::Null, -32600, "Batches aren't supported; send one message at a time");
        }
        let method = message.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let id = match message.get("id") {
            Some(id) if !method.is_empty() => id.clone(),
            Some(id) if message.get("result").is_none() && message.get("error").is_none() => return error(id, -32600, "Invalid request"),
            _ => {
                // A notification. Cancelling stops a running recording or scroll.
                if method == "notifications/cancelled" {
                    self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                return None;
            }
        };
        match method.as_str() {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
                result(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {
                            "tools": {"listChanged": false},
                            "resources": {"subscribe": false, "listChanged": false},
                            "prompts": {"listChanged": false},
                            "completions": {},
                            "logging": {},
                        },
                        "serverInfo": {"name": "snagster", "title": "Snagster", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(&id, json!({})),
            "tools/list" => result(&id, tool_list()),
            "prompts/list" => result(&id, prompts_list()),
            "prompts/get" => match prompts_get(&params) {
                Ok(v) => result(&id, v),
                Err((c, m)) => error(&id, c, &m),
            },
            "resources/list" => {
                let mut list = vec![
                    json!({"uri": "snagster://settings", "name": "settings", "title": "Snagster's settings", "mimeType": "application/json"}),
                    json!({"uri": "snagster://library", "name": "library", "title": "The newest captures in the library", "mimeType": "application/json"}),
                ];
                if let Ok(ws) = self.ws.try_lock()
                    && let Some((name, _)) = ws.current_png()
                {
                    list.insert(0, json!({"uri": "snagster://picture/current", "name": "current-picture", "title": name, "mimeType": "image/png"}));
                }
                result(&id, json!({ "resources": list }))
            }
            "resources/templates/list" => result(&id, json!({"resourceTemplates": []})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or_default();
                match uri {
                    "snagster://settings" => {
                        let s = serde_json::to_string_pretty(&crate::settings::Settings::load(false)).unwrap_or_default();
                        result(&id, json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": s}]}))
                    }
                    "snagster://library" => {
                        let s = crate::settings::Settings::load(false);
                        let items: Vec<Value> = library::scan(std::path::Path::new(&s.library_folder))
                            .into_iter()
                            .take(50)
                            .map(|i| json!({"path": i.path, "title": i.meta.title, "program": i.meta.app, "created": i.meta.created, "tags": i.meta.tags}))
                            .collect();
                        result(&id, json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(&items).unwrap_or_default()}]}))
                    }
                    "snagster://picture/current" => {
                        let png = self.ws.lock().ok().and_then(|ws| ws.current_png());
                        match png {
                            Some((_, img)) => result(&id, json!({"contents": [{"uri": uri, "mimeType": "image/png", "blob": img.get("data").cloned().unwrap_or_default()}]})),
                            None => error(&id, -32002, "There is no current picture yet"),
                        }
                    }
                    _ => error(&id, -32002, &format!("Resource not found: {uri}")),
                }
            }
            "completion/complete" => result(&id, json!({"completion": {"values": [], "total": 0, "hasMore": false}})),
            "logging/setLevel" => result(&id, json!({})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let mut ws = match self.ws.lock() {
                    Ok(ws) => ws,
                    Err(p) => p.into_inner(),
                };
                let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ws.call(&name, &args)))
                    .unwrap_or_else(|_| Err("the tool stopped unexpectedly (Snagster kept running)".into()));
                match out {
                    Ok(content) => {
                        // A JSON answer also goes out as structured content.
                        let structured = content
                            .first()
                            .filter(|_| content.len() == 1)
                            .and_then(|c| c.get("text"))
                            .and_then(Value::as_str)
                            .and_then(|t| serde_json::from_str::<Value>(t).ok())
                            .filter(Value::is_object);
                        match structured {
                            Some(s) => result(&id, json!({"content": content, "structuredContent": s, "isError": false})),
                            None => result(&id, json!({"content": content, "isError": false})),
                        }
                    }
                    Err(e) => result(&id, json!({"content": [text(e)], "isError": true})),
                }
            }
            _ => error(&id, -32601, &format!("Method not found: {method}")),
        }
    }
}

/// Serve MCP over stdin/stdout until stdin closes. Tool calls run on their own thread, so a
/// cancel (or a ping) is answered while a long recording runs.
pub fn serve_stdio() {
    let session = std::sync::Arc::new(Session::default());
    let out = std::sync::Arc::new(std::sync::Mutex::new(std::io::stdout()));
    fn send(out: &std::sync::Mutex<std::io::Stdout>, line: &str) {
        if let Ok(mut o) = out.lock() {
            let _ = writeln!(o, "{line}");
            let _ = o.flush();
        }
    }
    let stdin = std::io::stdin();
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        workers.retain(|w| !w.is_finished());
        if line.contains("\"tools/call\"") {
            let (s, o) = (session.clone(), out.clone());
            let spawned = std::thread::Builder::new().name("snagster-mcp-call".into()).spawn(move || {
                if let Some(reply) = s.handle(&line) {
                    send(&o, &reply);
                }
            });
            if let Ok(h) = spawned {
                workers.push(h);
            }
            continue;
        }
        if let Some(reply) = session.handle(&line) {
            send(&out, &reply);
        }
    }
    // Finish what's running before going away.
    for w in workers {
        let _ = w.join();
    }
}

/// The command-line commands: `snagster <command> ...`. Returns None when the arguments are
/// not a command (so the window opens), or the exit code.
pub fn run_cli(args: &[String]) -> Option<i32> {
    let cmd = args.first()?.as_str();
    let mut ws = Workspace::default();
    let out_path = |args: &[String]| args.windows(2).find(|w| w[0] == "-o" || w[0] == "--out").map(|w| w[1].clone());
    let show = |r: Outcome| -> i32 {
        match r {
            Ok(items) => {
                for i in items {
                    if let Some(t) = i.get("text").and_then(Value::as_str) {
                        println!("{t}");
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("Snagster: {e}");
                1
            }
        }
    };
    let save = |ws: &mut Workspace, p: Option<String>| -> i32 {
        match p {
            Some(p) => show(ws.call("save", &json!({"path": p}))),
            None => 0,
        }
    };
    let code = match cmd {
        "help" | "--help" | "-h" => {
            println!(
                "Snagster {}\n\nUse: snagster [command]\n\n  (no command)                          open the window\n  --mcp                                 MCP server for AI apps (stdin/stdout)\n  capture screen [-o out.png]           the screen under the mouse\n  capture all [-o out.png]              all screens\n  capture region X Y W H [-o out.png]   a desktop rectangle\n  capture window TITLE [-o out.png]     a window whose title contains TITLE\n  ocr IMAGE                             print the text in a picture\n  redact IMAGE -o OUT                   blur emails, phone numbers... and save\n  effect IMAGE EFFECT -o OUT            apply an effect ({})\n  convert IMAGE -o OUT                  save in another format (by extension)\n  record [SECONDS] [-o out.mp4]         record the screen to a video (default 10 s)\n      [--fps 1-120] [--format mp4|hevc|wmv] [--quality 1-4]\n      [--region X Y W H] [--software] [--no-cursor]\n  encoders                              list the video encoders\n  windows | monitors                    list windows or screens\n  tools                                 list the AI tools\n  call TOOL [JSON]                      run one AI tool, e.g. call list_windows\n",
                env!("CARGO_PKG_VERSION"),
                effect_names().join(", ")
            );
            0
        }
        "tools" => {
            for (n, d) in tool_names() {
                println!("{n:<20} {d}");
            }
            0
        }
        "windows" => show(ws.call("list_windows", &json!({}))),
        "encoders" => show(ws.call("list_video_encoders", &json!({}))),
        "record" => {
            // snagster record [SECONDS] [--fps N] [--format mp4|hevc|wmv] [--quality 1-4] [--software] [--region X Y W H] [-o FILE]
            let flag = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
            let mut a = json!({"seconds": args.get(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or(10.0)});
            if let Some(v) = flag("--fps").and_then(|s| s.parse::<f64>().ok()) {
                a["fps"] = json!(v);
            }
            if let Some(v) = flag("--format") {
                a["format"] = json!(v);
            }
            if let Some(v) = flag("--quality").and_then(|s| s.parse::<f64>().ok()) {
                a["quality"] = json!(v);
            }
            if args.iter().any(|s| s == "--software") {
                a["hardware"] = json!(false);
            }
            if args.iter().any(|s| s == "--no-cursor") {
                a["cursor"] = json!(false);
            }
            if let Some(i) = args.iter().position(|s| s == "--region") {
                let v: Vec<f64> = args.iter().skip(i + 1).take(4).filter_map(|s| s.parse().ok()).collect();
                if v.len() == 4 {
                    a["x"] = json!(v[0]);
                    a["y"] = json!(v[1]);
                    a["width"] = json!(v[2]);
                    a["height"] = json!(v[3]);
                }
            }
            if let Some(p) = out_path(args) {
                a["path"] = json!(p);
            }
            show(ws.call("record_video", &a))
        }
        "monitors" => show(ws.call("list_monitors", &json!({}))),
        "call" => {
            let tool = args.get(1).map(|s| s.as_str()).unwrap_or("");
            let a: Value = args.get(2).and_then(|s| serde_json::from_str(s).ok()).unwrap_or(json!({}));
            show(ws.call(tool, &a))
        }
        "capture" => {
            let what = args.get(1).map(|s| s.as_str()).unwrap_or("screen");
            let r = match what {
                "all" => ws.call("capture_screen", &json!({"all_screens": true})),
                "region" => {
                    let v: Vec<f64> = args.iter().skip(2).take(4).filter_map(|s| s.parse().ok()).collect();
                    if v.len() < 4 {
                        Err("capture region needs X Y WIDTH HEIGHT".into())
                    } else {
                        ws.call("capture_region", &json!({"x": v[0], "y": v[1], "width": v[2], "height": v[3]}))
                    }
                }
                "window" => ws.call("capture_window", &json!({"title": args.get(2).cloned().unwrap_or_default()})),
                _ => ws.call("capture_screen", &json!({})),
            };
            let c = show(r);
            if c == 0 {
                let p = out_path(args).or_else(|| Some(format!("snagster-{}.png", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"))));
                save(&mut ws, p)
            } else {
                c
            }
        }
        "ocr" | "redact" | "effect" | "convert" => {
            let Some(img) = args.get(1) else {
                eprintln!("Snagster: {cmd} needs a picture file");
                return Some(2);
            };
            if show(ws.call("open_image", &json!({"path": img}))) != 0 {
                return Some(1);
            }
            match cmd {
                "ocr" => show(ws.call("read_text", &json!({}))),
                "redact" => {
                    let c = show(ws.call("smart_redact", &json!({})));
                    if c == 0 { save(&mut ws, out_path(args)) } else { c }
                }
                "effect" => {
                    let e = args.get(2).cloned().unwrap_or_default();
                    let c = show(ws.call("apply_effect", &json!({"effect": e})));
                    if c == 0 { save(&mut ws, out_path(args)) } else { c }
                }
                _ => save(&mut ws, out_path(args)),
            }
        }
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(s: &Session, v: Value) -> Value {
        serde_json::from_str(&s.handle(&v.to_string()).expect("a reply")).expect("JSON")
    }

    fn call(s: &Session, id: u64, name: &str, args: Value) -> Value {
        ask(s, json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}}))["result"].clone()
    }

    #[test]
    fn tools_are_unique_and_described() {
        let list = tool_list();
        let tools = list["tools"].as_array().unwrap();
        assert!(tools.len() >= 50);
        let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n, "two tools share a name");
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(!t["description"].as_str().unwrap().is_empty());
            assert!(t["annotations"]["readOnlyHint"].is_boolean());
        }
    }

    #[test]
    fn the_protocol_answers_and_refuses_politely() {
        let s = Session::default();
        let init = ask(&s, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}));
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let init = ask(&s, json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}));
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert_eq!(ask(&s, json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}))["result"], json!({}));
        assert_eq!(ask(&s, json!({"jsonrpc": "2.0", "id": 4, "method": "nope"}))["error"]["code"], -32601);
        assert!(s.handle("{not json").unwrap().contains("-32700"));
        assert!(s.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(s.handle(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#).unwrap().contains("-32600"));
        let p = ask(&s, json!({"jsonrpc": "2.0", "id": 5, "method": "prompts/get", "params": {"name": "record_demo", "arguments": {"seconds": "7"}}}));
        assert!(p["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("7 seconds"));
    }

    #[test]
    fn a_picture_can_be_opened_marked_cropped_and_saved() {
        let dir = std::env::temp_dir().join(format!("snagster-mcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.png");
        image::RgbaImage::from_pixel(300, 200, image::Rgba([240, 240, 250, 255])).save(&src).unwrap();
        let s = Session::default();
        let r = call(&s, 1, "open_image", json!({"path": src}));
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(call(&s, 2, "add_mark", json!({"kind": "box", "x": 10, "y": 10, "x2": 100, "y2": 80}))["isError"], false);
        assert_eq!(call(&s, 3, "add_mark", json!({"kind": "step", "x": 150, "y": 50}))["isError"], false);
        let marks = call(&s, 4, "list_marks", json!({}));
        assert!(marks["content"][0]["text"].as_str().unwrap().contains('1'));
        assert_eq!(call(&s, 5, "crop", json!({"x": 0, "y": 0, "width": 200, "height": 150}))["isError"], false);
        assert_eq!(call(&s, 6, "undo", json!({}))["isError"], false);
        assert_eq!(call(&s, 7, "redo", json!({}))["isError"], false);
        assert_eq!(call(&s, 8, "clear_marks", json!({}))["isError"], false);
        let out = dir.join("out.jpg");
        assert_eq!(call(&s, 9, "save", json!({"path": out}))["isError"], false);
        let img = image::open(&out).unwrap();
        assert_eq!((img.width(), img.height()), (200, 150));
        let bad = call(&s, 10, "crop", json!({"x": 0, "y": 0, "width": -5, "height": 10}));
        assert_eq!(bad["isError"], true);
        assert_eq!(call(&s, 11, "no_such_tool", json!({}))["isError"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colours_parse() {
        assert_eq!(parse_color(Some("#00FF00")), Color32::from_rgb(0, 255, 0));
        assert_eq!(parse_color(Some("blue")), Color32::from_rgb(0, 144, 255));
    }
}
