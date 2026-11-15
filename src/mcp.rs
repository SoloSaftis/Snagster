//! Snagster for AI apps: an MCP server (JSON-RPC 2.0 over stdin/stdout, one message per line)
//! and the same tools as command-line commands. Start it with `snagster --mcp`.
//!
//! The tools work on one "current picture" kept in memory: capture or open one, add marks,
//! blur, crop, apply effects, read its text, then save or copy it.

use crate::capture::{self, CaptureKind, CutShape};
use crate::doc::{self, Doc, Shape};
use crate::settings::Settings;
use crate::{effects, export, library, ocr, render};
use base64::Engine as _;
use eframe::egui::{Color32, Pos2, Rect, pos2, vec2};
use image::RgbaImage;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The first protocol version with `resource_link` content.
const LINKS_SINCE: &str = "2025-06-18";

pub const INSTRUCTIONS: &str = "Snagster captures the screen, marks up pictures and records video. \
Pictures: capture_screen, capture_region, capture_window, capture_active_window, capture_last_area, capture_scrolling, \
run_preset, paste_from_clipboard, combine_images or open_image make the current picture (captures take delay_s and a \
cut-out shape, and like the window they add the mouse pointer and keep a copy in the library when the settings say so). \
Then add_mark / update_mark / remove_mark (arrows, boxes, text, callouts, steps, pen, pictures...), blur_area, crop, \
resize, trim, set_canvas, apply_effect, undo and redo; view_picture shows it; save, quick_save, copy_to_clipboard, \
library_add or open_in_snagster keep it. Text recognition (Windows only): read_text, read_screen_text, smart_redact, \
simplify. Recording: record_video (MP4 with H.264 everywhere; HEVC and WMV on Windows; 1-120 frames per second; sound \
on Windows), record_gif, and Studio scenes (studio_scenes, studio_add_scene, studio_add_source, studio_update_source, \
studio_record). Guides: make_guide (PDF, web page, Word or one tall picture). Library: library_search, library_tag, \
library_delete. Live streaming: stream_services, stream_start (Twitch, YouTube, Kick, Facebook, Restream or any RTMP/RTMPS \
server; the screen, a window, an area or the Studio scene; the stream key saved in Snagster is used and never shown), \
stream_status and stream_stop. Defaults come from Snagster's settings (get_settings, set_setting). Long calls send progress when the \
request has a progressToken and stop early when cancelled. Coordinates are picture pixels from the top-left corner \
(desktop pixels for captures and recordings). Colours are #RRGGBB or a name (red, orange, yellow, green, blue, purple, \
black, white).";

/// Reports how far a running call is: done so far, the total when known, and what it's doing.
pub type Progress = Arc<dyn Fn(f64, Option<f64>, &str) + Send + Sync>;

/// Sends one JSON-RPC message (a notification) to the AI app while a call runs.
pub type Notify = Arc<dyn Fn(String) + Send + Sync>;

/// The picture the tools work on.
#[derive(Default)]
pub struct Workspace {
    doc: Option<Doc>,
    name: String,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    /// Set to stop a running recording (an AI app cancelled the call).
    pub stop: Arc<AtomicBool>,
    /// Where the running call reports progress (when the app asked for it).
    progress: Option<Progress>,
    /// The running call's answer as JSON, when it has one beyond its text.
    structured: Option<Value>,
    /// Settings used instead of the saved file (tests).
    fixed: Option<Settings>,
    /// The last desktop area captured in this session, for capture_last_area.
    last_area: Option<(i32, i32, u32, u32)>,
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

/// Add `extra`'s properties to the schema properties `base`.
fn with(mut base: Value, extra: Value) -> Value {
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}

/// The options every capture takes.
fn capture_props() -> Value {
    json!({
        "delay_s": {"type": "number", "description": "Wait this many seconds first (0-60), e.g. to open a menu."},
        "shape": {"type": "string", "enum": SHAPE_NAMES, "description": "Cut the picture to this shape (polygon and freehand need points)."},
        "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "[[x, y], ...] in picture pixels, for the polygon and freehand shapes."},
        "include_cursor": {"type": "boolean", "description": "Add the mouse pointer as a movable mark (default: the setting)."},
        "library": {"type": "boolean", "description": "Also keep it in Snagster's library (default: the setting \"add to the library\")."}
    })
}

const SHAPE_NAMES: [&str; 8] = ["rectangle", "ellipse", "rounded", "triangle", "star", "heart", "polygon", "freehand"];

fn base_tools() -> Vec<Tool> {
    let n = |d: &str| json!({"type": "number", "description": d});
    let s = |d: &str| json!({"type": "string", "description": d});
    let b = |d: &str| json!({"type": "boolean", "description": d});
    let rect = || json!({"x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number"}, "height": {"type": "number"}});
    let formats = crate::video::VideoFormat::video_names();
    let format_help = if formats.len() > 1 { "mp4 (H.264), hevc (MP4 with H.265) or wmv; default: the setting." } else { "mp4 (H.264); default mp4." };
    vec![
        Tool { name: "list_monitors", description: "The screens: id, name, position and size in desktop pixels.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "list_windows", description: "Visible windows: id, title, program, position and size.", schema: obj(json!({}), &[]), edits: false },
        Tool {
            name: "capture_screen",
            description: "Capture a screen (the one under the mouse, a monitor by id, or all screens) as the current picture. Returns a preview.",
            schema: obj(with(json!({"monitor_id": n("A monitor id from list_monitors."), "all_screens": b("Every screen in one picture.")}), capture_props()), &[]),
            edits: true,
        },
        Tool {
            name: "capture_region",
            description: "Capture a desktop rectangle (desktop pixels) as the current picture. capture_last_area repeats it.",
            schema: obj(with(rect(), capture_props()), &["x", "y", "width", "height"]),
            edits: true,
        },
        Tool {
            name: "capture_window",
            description: "Capture one window, by id or by part of its title, as the current picture.",
            schema: obj(with(json!({"id": n("A window id from list_windows."), "title": s("Part of the window title.")}), capture_props()), &[]),
            edits: true,
        },
        Tool { name: "open_image", description: "Open a picture file (PNG, JPEG, BMP, GIF, WebP, TIFF) or a .snag project as the current picture.", schema: obj(json!({"path": s("Full path.")}), &["path"]), edits: true },
        Tool { name: "picture_info", description: "Size of the current picture and its marks.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "view_picture", description: "The current picture with its marks, as an image (scaled to at most max_size pixels).", schema: obj(json!({"max_size": n("Longest side, default 1024.")}), &[]), edits: false },
        Tool {
            name: "add_mark",
            description: "Add a mark. kind: arrow, line, box, oval, highlight, pen, text, callout, step, stamp, pointer, spotlight, magnify, image. Arrows/lines go from (x, y) to (x2, y2); boxes, ovals, highlight and spotlight cover (x, y)-(x2, y2); a pen stroke follows points; text, callout, step, stamp, pointer and magnify are placed at (x, y) (a callout points at (x2, y2)); an image puts a picture file at (x, y), sized to (x2, y2) or its own size.",
            schema: obj(
                json!({
                    "kind": {"type": "string", "enum": MARK_KINDS},
                    "x": {"type": "number"}, "y": {"type": "number"}, "x2": {"type": "number"}, "y2": {"type": "number"},
                    "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "[[x, y], ...] for a pen stroke (two or more)."},
                    "text": s("Text for text and callout; the emoji for stamp."),
                    "path": s("Picture file for kind image."),
                    "color": s("#RRGGBB or a colour name. Default red."),
                    "width": n("Line width in pixels, default 6."),
                    "size": n("Text size, stamp size, step radius, pointer size or magnify radius."),
                    "fill": b("Fill boxes and ovals; a background behind text."),
                    "shadow": b("A soft shadow."),
                    "heads": n("Arrow heads: 0 none, 1 at the end (default), 2 at both ends."),
                    "radius": n("Rounded corners of a box, in pixels."),
                    "font": s("Font name for text and callouts (empty: Snagster's)."),
                    "bold": b("Bold text."), "italic": b("Italic text."),
                    "align": {"type": "string", "enum": ["left", "center", "right"], "description": "Text alignment."},
                    "outline": b("An outline around text letters (text default on) or a callout bubble (default off)."),
                    "callout_shape": {"type": "string", "enum": ["rounded", "rectangle", "oval"], "description": "The callout bubble."},
                    "number": n("Number for a step; default the next one."),
                    "style": {"type": "string", "enum": ["numbers", "upper", "lower", "roman"], "description": "How a step shows its number: 1, A, a or i."},
                    "zoom": n("How much a magnifier enlarges, 1-8 (default 2).")
                }),
                &["kind"],
            ),
            edits: true,
        },
        Tool {
            name: "update_mark",
            description: "Change a mark (by its number from list_marks): move it by dx/dy or to x/y (its top-left corner), scale it, or change its colour, width, fill, shadow, text, font, arrow heads, box corners, step number/style or magnifier zoom.",
            schema: obj(
                json!({
                    "index": n("Mark number."),
                    "dx": n("Move right by this many pixels."), "dy": n("Move down by this many pixels."),
                    "x": n("Move so its left edge is here."), "y": n("Move so its top edge is here."),
                    "scale": n("Make it this many times bigger (0.05-20)."),
                    "color": s("#RRGGBB or a colour name."), "width": n("Line width."),
                    "fill": b("Fill / text background."), "shadow": b("Shadow."),
                    "text": s("New text (text, callout) or emoji (stamp)."),
                    "font": s("Font name."), "bold": b("Bold."), "italic": b("Italic."),
                    "align": {"type": "string", "enum": ["left", "center", "right"]},
                    "outline": b("Outline."),
                    "callout_shape": {"type": "string", "enum": ["rounded", "rectangle", "oval"]},
                    "heads": n("Arrow heads 0, 1 or 2."), "radius": n("Box corner radius."),
                    "number": n("Step number."), "style": {"type": "string", "enum": ["numbers", "upper", "lower", "roman"]},
                    "zoom": n("Magnifier zoom 1-8.")
                }),
                &["index"],
            ),
            edits: true,
        },
        Tool { name: "list_marks", description: "The marks on the current picture, numbered from 0, with their boxes.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "remove_mark", description: "Remove a mark by its number from list_marks.", schema: obj(json!({"index": n("Mark number.")}), &["index"]), edits: true },
        Tool {
            name: "blur_area",
            description: "Hide part of the picture: pixelate, blur or a solid box (default: the blur setting), in the setting's block size.",
            schema: obj(with(rect(), json!({"mode": {"type": "string", "enum": ["pixelate", "blur", "solid"]}, "color": s("For solid; default black.")})), &["x", "y", "width", "height"]),
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
        Tool { name: "read_text", description: "Read the text in the current picture (Windows only: Windows text recognition): every line with its box.", schema: obj(json!({}), &[]), edits: false },
        Tool {
            name: "smart_redact",
            description: "Find private details in the current picture and pixelate them (Windows only: uses Windows text recognition): emails, phone numbers, card numbers, street addresses, IP addresses and web addresses. Default: the kinds chosen in Snagster's settings (web addresses are off unless switched on).",
            schema: obj(json!({"kinds": {"type": "array", "items": {"type": "string", "enum": ocr::RedactKinds::NAMES}, "description": "Only these kinds instead of the settings'."}}), &[]),
            edits: true,
        },
        Tool { name: "undo", description: "Undo the last change to the current picture.", schema: obj(json!({}), &[]), edits: true },
        Tool {
            name: "save",
            description: "Save the current picture (an existing file is replaced). The extension picks the format: png, jpg (the setting's quality), bmp, gif, webp, tiff, pdf, svg, docx, or snag (keeps marks editable).",
            schema: obj(json!({"path": s("Full path with extension.")}), &["path"]),
            edits: false,
        },
        Tool { name: "copy_to_clipboard", description: "Copy the current picture (with marks) to the clipboard.", schema: obj(json!({}), &[]), edits: false },
        Tool {
            name: "record_gif",
            description: "Record the screen under the mouse (or a desktop rectangle) as an animated GIF for some seconds. Frame rate, largest width and the pointer come from the GIF settings.",
            schema: obj(
                with(
                    rect(),
                    json!({
                        "seconds": n("1-120, default 5."),
                        "fps": n("1-30, default: the GIF setting."),
                        "max_width": n("Shrink wider recordings to this width (default: the setting)."),
                        "cursor": b("Draw the mouse pointer (default: the setting)."),
                        "path": s("Where to save the .gif; default a new file in the recordings folder.")
                    }),
                ),
                &[],
            ),
            edits: false,
        },
        Tool {
            name: "record_video",
            description: "Record the screen under the mouse (or a desktop rectangle, or one monitor) to a video file for some seconds, at 1-120 frames per second, with sound on Windows. Uses the graphics chip's encoder (Intel, AMD or NVIDIA) when there is one. Everything not given comes from Snagster's video settings. Returns the file, its length, frames and the encoder used.",
            schema: obj(
                json!({
                    "seconds": n("1-600."),
                    "fps": n("Frames per second, 1-120; default: the setting."),
                    "format": {"type": "string", "enum": formats, "description": format_help},
                    "quality": n("1 small file, 2 good, 3 high, 4 best; default: the setting."),
                    "hardware": b("Use the graphics chip's encoder (default: the setting)."),
                    "cursor": b("Draw the mouse pointer (default: the setting)."),
                    "audio": {"type": "string", "enum": ["none", "system", "microphone", "both"], "description": "Sound to record (Windows, MP4 only); default: the setting."},
                    "microphone": s("Microphone id from list_audio_devices (default: the setting, or the computer's default)."),
                    "monitor_id": n("Record this monitor (from list_monitors)."),
                    "x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number"}, "height": {"type": "number"},
                    "path": s("Where to save it; default a new file in the recordings folder (the video_folder setting).")
                }),
                &["seconds"],
            ),
            edits: false,
        },
        Tool { name: "list_video_encoders", description: "The video encoders this computer has (graphics-chip and software) for H.264, HEVC and WMV. To use one for recording and streaming: set_setting video_encoder to its name (\"\" automatic: the best that works; a choice that can't start falls back by itself).", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "library_search", description: "Search Snagster's capture library by title, program, tag or date.", schema: obj(json!({"query": s("Words to find; empty lists everything."), "limit": n("Default 20.")}), &[]), edits: false },
    ]
}

const MARK_KINDS: [&str; 14] = ["arrow", "line", "box", "oval", "highlight", "pen", "text", "callout", "step", "stamp", "pointer", "spotlight", "magnify", "image"];

/// Tools that change something outside the current picture (files, clipboard, settings, screen).
const SIDE: [&str; 26] = [
    "save", "quick_save", "copy_to_clipboard", "record_gif", "record_video", "make_guide", "library_add", "library_delete", "library_tag",
    "set_setting", "set_language", "open_in_snagster", "studio_add_source", "studio_remove_source", "studio_update_source",
    "studio_add_scene", "studio_remove_scene", "studio_set_current", "studio_record", "capture_scrolling", "paste_from_clipboard",
    "capture_screen", "capture_region", "capture_window", "stream_start", "stream_stop",
];

/// Tools that may delete or replace something (a mark, a source or scene, a library picture,
/// a setting, or a file at a given path).
const DESTRUCTIVE: [&str; 11] =
    ["remove_mark", "clear_marks", "studio_remove_source", "studio_remove_scene", "library_delete", "set_setting", "save", "make_guide", "record_video", "record_gif", "studio_record"];

/// Tools that change something but leave the same result when called again with the same
/// arguments.
const IDEMPOTENT: [&str; 11] = ["stream_stop", "save", "set_setting", "set_language", "studio_set_current", "library_tag", "open_image", "clear_marks", "resize", "set_canvas", "copy_to_clipboard"];

/// What a tool's structured answer looks like, for the tools that always give one.
fn output_schema(name: &str) -> Option<Value> {
    let num = json!({"type": "number"});
    let st = json!({"type": "string"});
    let bo = json!({"type": "boolean"});
    let strings = json!({"type": "array", "items": {"type": "string"}});
    let props = |p: Value| json!({"type": "object", "properties": p});
    let items = |item: Value| json!({"type": "object", "properties": {"items": {"type": "array", "items": item}}, "required": ["items"]});
    let text_lines = || {
        json!({
            "type": "object",
            "properties": {"text": st, "lines": {"type": "array", "items": props(json!({"text": st, "x": num, "y": num, "width": num, "height": num}))}},
            "required": ["text", "lines"]
        })
    };
    Some(match name {
        "list_monitors" => items(props(json!({"id": num, "name": st, "x": num, "y": num, "width": num, "height": num, "primary": bo}))),
        "list_windows" => items(props(json!({"id": num, "title": st, "program": st, "x": num, "y": num, "width": num, "height": num}))),
        "library_search" => items(props(json!({"path": st, "title": st, "program": st, "kind": st, "created": st, "tags": strings}))),
        "list_languages" => items(props(json!({"code": st, "name": st, "native": st}))),
        "list_presets" => items(props(json!({"number": num, "name": st, "mode": st, "delay_s": num, "effects": strings, "hotkey": st}))),
        "list_video_encoders" => items(props(json!({"name": st, "codec": st, "hardware": bo, "vendor": st}))),
        "list_marks" => items(props(json!({"index": num, "kind": st, "description": st, "x": num, "y": num, "width": num, "height": num}))),
        "read_text" | "read_screen_text" => text_lines(),
        "list_recordings" => json!({"type": "object", "properties": {"folder": st, "recordings": {"type": "array", "items": props(json!({"path": st, "kb": num}))}}, "required": ["folder", "recordings"]}),
        "video_info" => json!({"type": "object", "properties": {"frames": num, "seconds": num, "width": num, "height": num}, "required": ["frames", "seconds", "width", "height"]}),
        "list_audio_devices" => json!({"type": "object", "properties": {"sound_recording": bo, "microphones": {"type": "array", "items": {"type": "object"}}}, "required": ["sound_recording", "microphones"]}),
        "app_info" => json!({"type": "object", "properties": {"version": st, "system": st, "architecture": st, "features": {"type": "object"}, "folders": {"type": "object"}}, "required": ["version", "system"]}),
        "studio_scenes" => json!({"type": "object", "properties": {"scenes": {"type": "array", "items": {"type": "object"}}, "current": num, "width": num, "height": num, "fps": num}, "required": ["scenes", "current"]}),
        "library_tag" => json!({"type": "object", "properties": {"path": st, "title": st, "tags": strings}, "required": ["path", "title", "tags"]}),
        "stream_services" => items(props(json!({"service": st, "name": st, "url": st, "key_help": st, "max_width": num, "max_height": num, "max_fps": num, "max_video_kbps": num, "max_pixel_rate": num, "keyframe_s": num, "audio_kbps": num, "hevc": bo}))),
        "stream_status" => json!({"type": "object", "properties": {"phase": st, "live": bo, "state": st, "uptime_s": num, "bitrate_bps": num, "frames_sent": num, "frames_dropped": num, "reconnects": num, "service": st, "address": st, "source": st, "width": num, "height": num, "fps": num, "also": {"type": "array", "items": st}, "outputs": {"type": "array", "items": {"type": "object", "properties": {"service": st, "address": st, "state": st, "bitrate_bps": num, "frames_sent": num, "frames_dropped": num, "reconnects": num}}}}, "required": ["phase", "live"]}),
        _ => return None,
    })
}

/// The `tools/list` result, with a title and safety hints for every tool.
pub fn tool_list() -> Value {
    let tools: Vec<Value> = tools()
        .into_iter()
        .map(|t| {
            let read_only = !t.edits && !SIDE.contains(&t.name);
            let mut title: String = t.name.replace('_', " ");
            if let Some(c) = title.get(0..1) {
                title = c.to_uppercase() + &title[1..];
            }
            let mut v = json!({
                "name": t.name,
                "title": title,
                "description": t.description,
                "inputSchema": t.schema,
                "annotations": {
                    "title": title,
                    "readOnlyHint": read_only,
                    "destructiveHint": DESTRUCTIVE.contains(&t.name),
                    "idempotentHint": read_only || IDEMPOTENT.contains(&t.name),
                    "openWorldHint": matches!(t.name, "check_for_update" | "stream_start"),
                },
            });
            if let Some(o) = output_schema(t.name) {
                v["outputSchema"] = o;
            }
            v
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

/// A list position (mark, source or scene number), 0 or more. A plain `as usize` would turn
/// -1 into 0 and act on the first one.
fn index_arg(a: &Value, k: &str) -> Result<usize, String> {
    let v = need(a, k)?;
    if v < 0.0 {
        return Err(format!("\"{k}\" must be 0 or more"));
    }
    Ok(v as usize)
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

fn bool_arg(a: &Value, k: &str) -> Option<bool> {
    a.get(k).and_then(Value::as_bool)
}

fn str_of<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

/// A `file://` address for a path (made absolute), for resource links.
pub fn file_uri(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s);
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A link to a file Snagster wrote, as MCP `resource_link` content.
fn link(p: &Path) -> Value {
    let mut v = json!({
        "type": "resource_link",
        "uri": file_uri(p),
        "name": p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        "mimeType": export::mime_type(p),
    });
    if let Ok(m) = std::fs::metadata(p) {
        v["size"] = json!(m.len());
    }
    v
}

/// The answer for a saved file: the message (for every app) and a link to it (newer apps).
fn saved(p: &Path, message: String) -> Vec<Value> {
    vec![text(message), link(p)]
}

/// A list of [x, y] pairs (or {"x", "y"} objects).
fn points_arg(a: &Value, k: &str) -> Result<Option<Vec<Pos2>>, String> {
    let Some(list) = a.get(k) else { return Ok(None) };
    let list = list.as_array().ok_or_else(|| format!("\"{k}\" is a list of [x, y] pairs"))?;
    let mut out = Vec::new();
    for p in list {
        let (x, y) = match p {
            Value::Array(xy) if xy.len() == 2 => (xy[0].as_f64(), xy[1].as_f64()),
            Value::Object(_) => (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)),
            _ => (None, None),
        };
        match (x, y) {
            (Some(x), Some(y)) => out.push(pos2(x as f32, y as f32)),
            _ => return Err(format!("\"{k}\" is a list of [x, y] pairs")),
        }
    }
    Ok(Some(out))
}

/// A cut-out shape by name.
fn cut_shape(s: &str) -> Option<CutShape> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "rectangle" | "rect" | "box" => CutShape::Rect,
        "ellipse" | "oval" | "circle" => CutShape::Ellipse,
        "rounded" | "rounded rectangle" => CutShape::Rounded,
        "triangle" => CutShape::Triangle,
        "star" => CutShape::Star,
        "heart" => CutShape::Heart,
        "polygon" => CutShape::Polygon,
        "freehand" | "lasso" => CutShape::Freehand,
        _ => return None,
    })
}

/// How a capture is taken and what happens after: read from the arguments before grabbing.
#[derive(Default, Debug)]
struct CaptureArgs {
    delay_s: f64,
    shape: Option<CutShape>,
    points: Vec<Pos2>,
    cursor: Option<bool>,
    library: Option<bool>,
}

impl CaptureArgs {
    fn parse(a: &Value) -> Result<CaptureArgs, String> {
        let delay_s = num(a, "delay_s").unwrap_or(0.0);
        if !(0.0..=60.0).contains(&delay_s) {
            return Err("delay_s is 0-60 seconds".into());
        }
        let shape = match str_of(a, "shape") {
            Some(s) => Some(cut_shape(s).ok_or_else(|| format!("shape is one of {}", SHAPE_NAMES.join(", ")))?),
            None => None,
        };
        let points = points_arg(a, "points")?.unwrap_or_default();
        if matches!(shape, Some(CutShape::Polygon | CutShape::Freehand)) && points.len() < 3 {
            return Err("the polygon and freehand shapes need three or more points".into());
        }
        Ok(CaptureArgs { delay_s, shape: shape.filter(|s| *s != CutShape::Rect), points, cursor: bool_arg(a, "include_cursor"), library: bool_arg(a, "library") })
    }
}

/// A picture taken from the screen, before the after-capture steps.
struct Grabbed {
    image: RgbaImage,
    /// Desktop position of its top-left pixel.
    origin: (i32, i32),
    /// Where the mouse pointer was, in desktop pixels.
    cursor: Option<(i32, i32)>,
    title: String,
    app: String,
}

impl Grabbed {
    fn from_shot(s: capture::Shot, title: Option<&str>) -> Grabbed {
        Grabbed { title: title.map(String::from).unwrap_or(s.title), image: s.image, origin: s.origin, cursor: s.cursor, app: s.app }
    }
}

/// A video format argument: one this system can make (GIFs come from record_gif).
fn video_format_arg(f: &str) -> Result<crate::video::VideoFormat, String> {
    use crate::video::VideoFormat;
    match VideoFormat::parse(f) {
        Some(VideoFormat::Gif) => Err("GIFs are made by record_gif".into()),
        Some(v) if v.available() => Ok(v),
        Some(v) => Err(format!("{} can't be made on this system; use mp4", v.label())),
        None => Err(format!("format is one of {}", VideoFormat::video_names().join(", "))),
    }
}

/// A `notifications/progress` message for `token`.
fn progress_note(token: &Value, done: f64, total: Option<f64>, message: &str) -> String {
    let mut p = json!({"progressToken": token, "progress": done, "message": message});
    if let Some(t) = total {
        p["total"] = json!(t);
    }
    json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": p}).to_string()
}

impl Workspace {
    /// A workspace that uses (and changes) these settings instead of the saved ones.
    #[cfg(test)]
    pub fn with_settings(s: Settings) -> Workspace {
        Workspace { fixed: Some(s), ..Default::default() }
    }

    fn settings(&self) -> Settings {
        self.fixed.clone().unwrap_or_else(|| Settings::load(false))
    }

    fn save_settings(&mut self, s: &Settings) {
        match &mut self.fixed {
            Some(f) => *f = s.clone(),
            None => s.save(),
        }
    }

    /// Tell the app how far the call is (when it asked).
    fn report(&self, done: f64, total: Option<f64>, what: &str) {
        if let Some(p) = &self.progress {
            p(done, total, what);
        }
    }

    /// Run `f` while reporting the seconds gone by (of `total`) about once a second.
    fn ticking<T>(&self, total: Option<f64>, what: &str, f: impl FnOnce() -> T) -> T {
        self.ticking_every(Duration::from_secs(1), total, what, f)
    }

    fn ticking_every<T>(&self, every: Duration, total: Option<f64>, what: &str, f: impl FnOnce() -> T) -> T {
        let Some(p) = self.progress.clone() else { return f() };
        let done = AtomicBool::new(false);
        std::thread::scope(|sc| {
            sc.spawn(|| {
                let t0 = Instant::now();
                let mut last = -1.0;
                loop {
                    let mut e = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;
                    if let Some(t) = total {
                        e = e.min(t);
                    }
                    // Progress only ever goes up.
                    if e > last {
                        p(e, total, what);
                        last = e;
                    }
                    let until = Instant::now() + every;
                    while Instant::now() < until {
                        if done.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20).min(every));
                    }
                }
            });
            let r = f();
            done.store(true, Ordering::Relaxed);
            r
        })
    }

    /// Wait before a capture; a cancel ends the wait.
    fn wait(&self, secs: f64) -> Result<(), String> {
        if secs <= 0.0 {
            return Ok(());
        }
        let t0 = Instant::now();
        self.ticking(Some(secs), "Waiting before the capture", || {
            while t0.elapsed().as_secs_f64() < secs {
                if self.stop.load(Ordering::Relaxed) {
                    return Err("cancelled".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        })
    }

    /// The after-capture steps the window runs too: cut to a shape, effects, the mouse pointer
    /// as a mark, a copy in the library. Then it is the current picture.
    fn finish_capture(&mut self, g: Grabbed, kind: &str, ca: &CaptureArgs, fx: &[effects::Effect]) -> Outcome {
        let s = self.settings();
        let mut img = g.image;
        if let Some(shape) = ca.shape {
            img = render::cut_to_shape(&img, shape, &ca.points);
        }
        if !fx.is_empty() {
            let opts = s.effect_options(effects::info_line(&[&g.app, &g.title]));
            for e in fx {
                img = effects::apply(*e, &img, &opts);
            }
        }
        let (w, h) = img.dimensions();
        let mut d = Doc::new(img.clone());
        if ca.cursor.unwrap_or(s.include_cursor)
            && let Some(c) = g.cursor
        {
            let p = pos2((c.0 - g.origin.0) as f32, (c.1 - g.origin.1) as f32);
            if p.x >= 0.0 && p.y >= 0.0 && p.x < w as f32 && p.y < h as f32 {
                d.shapes.push(Shape::Pointer { pos: p, size: doc::pointer_size(4.0) });
            }
        }
        let mut note = String::new();
        if ca.library.unwrap_or(s.after_library) {
            let meta = library::Meta { title: g.title.clone(), app: g.app.clone(), kind: kind.into(), ..Default::default() };
            note = match library::add(Path::new(&s.library_folder), &img, meta) {
                Ok(p) => format!(" Kept in the library: {}", p.display()),
                Err(e) => format!(" Not added to the library: {e}"),
            };
        }
        let mut out = self.set(d, &g.title)?;
        if !note.is_empty()
            && let Some(t) = out[0].get("text").and_then(Value::as_str)
        {
            out[0] = text(format!("{t}{note}"));
        }
        Ok(out)
    }

    /// Copy the current picture to the clipboard.
    fn copy_current(&mut self) -> Result<String, String> {
        let flat = render::compose(self.doc()?);
        let (w, h) = flat.dimensions();
        let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: flat.into_raw().into() };
        arboard::Clipboard::new().and_then(|mut c| c.set_image(data)).map_err(|e| e.to_string())?;
        Ok(format!("Copied the {w} x {h} picture."))
    }

    /// Save the current picture in the save folder, named by the file-name pattern.
    fn quick_save(&mut self) -> Outcome {
        let name = self.name.clone();
        let d = self.doc()?.clone();
        let mut s = self.settings();
        let p = s.next_save_path(&name);
        export::save_doc(&d, &name, &p, &s)?;
        // The %n counter moved on.
        self.save_settings(&s);
        Ok(saved(&p, format!("Saved {}", p.display())))
    }

    fn doc(&mut self) -> Result<&mut Doc, String> {
        self.doc.as_mut().ok_or_else(|| "There is no current picture yet: capture_screen or open_image first.".to_string())
    }

    fn set(&mut self, d: Doc, name: &str) -> Outcome {
        let (w, h) = d.image.dimensions();
        self.doc = Some(d);
        self.name = name.to_string();
        // A new picture: undo and redo belong to the old one.
        self.undo.clear();
        self.redo.clear();
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
        self.structured = None;
        self.run(name, a)
    }

    fn run(&mut self, name: &str, a: &Value) -> Outcome {
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
                let ca = CaptureArgs::parse(a)?;
                let (kind, label) = if bool_arg(a, "all_screens").unwrap_or(false) {
                    (CaptureKind::AllScreens, "All screens")
                } else if let Some(id) = num(a, "monitor_id") {
                    (CaptureKind::Monitor(id as u32), "Screen")
                } else {
                    (CaptureKind::Screen, "Screen")
                };
                self.wait(ca.delay_s)?;
                let shot = capture::grab(&kind, (0, 0))?;
                let title = if kind == CaptureKind::AllScreens { Some("All screens") } else { None };
                self.finish_capture(Grabbed::from_shot(shot, title), label, &ca, &[])
            }
            "capture_region" => {
                let r = rect_arg(a)?;
                let ca = CaptureArgs::parse(a)?;
                let area = (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32);
                self.wait(ca.delay_s)?;
                let cursor = capture::cursor_pos();
                let img = capture::grab_rect(area.0, area.1, area.2, area.3)?;
                self.last_area = Some(area);
                let app = capture::foreground_info().1;
                self.finish_capture(Grabbed { image: img, origin: (area.0, area.1), cursor, title: "Area".into(), app }, "Area", &ca, &[])
            }
            "capture_window" => {
                let ca = CaptureArgs::parse(a)?;
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
                self.wait(ca.delay_s)?;
                let shot = capture::grab(&CaptureKind::Window(w.id, w.label.clone()), (0, 0))?;
                let mut g = Grabbed::from_shot(shot, Some(&w.label));
                if g.app.is_empty() {
                    g.app = w.app.clone();
                }
                self.finish_capture(g, "Window", &ca, &[])
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
            "update_mark" => {
                let i = index_arg(a, "index")?;
                let mut what = Vec::new();
                self.change(|d| {
                    if i >= d.shapes.len() {
                        return Err("no mark with that number".into());
                    }
                    let mut d = d.clone();
                    what = update_shape(&mut d.shapes[i], a)?;
                    Ok(d)
                })?;
                let label = describe(&self.doc()?.shapes[i]);
                Ok(vec![text(format!("Mark {i} ({}): {label}.", what.join(", ")))])
            }
            "list_marks" => {
                let d = self.doc()?;
                let lines: Vec<String> = d.shapes.iter().enumerate().map(|(i, s)| format!("{i}: {}", describe(s))).collect();
                let items: Vec<Value> = d
                    .shapes
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let b = s.bounds();
                        json!({"index": i, "kind": kind_name(s), "description": describe(s), "x": b.min.x.round(), "y": b.min.y.round(), "width": b.width().round(), "height": b.height().round()})
                    })
                    .collect();
                self.structured = Some(json!({ "items": items }));
                Ok(vec![text(if lines.is_empty() { "No marks.".into() } else { lines.join("\n") })])
            }
            "remove_mark" => {
                let i = index_arg(a, "index")?;
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
                let s = self.settings();
                let mode = match a.get("mode").and_then(Value::as_str) {
                    Some("pixelate") => crate::settings::BlurMode::Pixelate,
                    Some("blur") => crate::settings::BlurMode::Blur,
                    Some("solid") => crate::settings::BlurMode::Solid,
                    _ => s.blur_mode,
                };
                let color = parse_color(a.get("color").and_then(Value::as_str).or(Some("black")));
                self.change(|d| d.redact(r, mode, s.blur_block, color).ok_or("that area is outside the picture".into()))?;
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
                let opts = self.settings().effect_options(effects::info_line(&[&self.name]));
                self.flatten_with(|i| effects::apply(e, i, &opts))?;
                Ok(vec![text(format!("{} applied.", e.label()))])
            }
            "read_text" => {
                let d = self.doc()?.clone();
                let lines = ocr::recognize(&render::compose(&d))?;
                let (plain, list) = text_lines(&lines);
                self.structured = Some(json!({"text": plain, "lines": list}));
                Ok(vec![text(plain), text(serde_json::to_string(&list).unwrap_or_default())])
            }
            "smart_redact" => {
                let d = self.doc()?.clone();
                let s = self.settings();
                let kinds = match a.get("kinds") {
                    Some(v) => {
                        let names: Vec<&str> = v.as_array().ok_or("kinds is a list of names")?.iter().filter_map(Value::as_str).collect();
                        ocr::RedactKinds::from_names(&names)?
                    }
                    None => ocr::RedactKinds::from_settings(&s),
                };
                let lines = ocr::recognize(&render::compose(&d))?;
                let Some((hidden, mut kinds)) = ocr::redact(&d, &lines, kinds, s.blur_block) else {
                    return Ok(vec![text("Nothing sensitive was found.")]);
                };
                let n = kinds.len();
                self.change(|_| Ok(hidden))?;
                kinds.dedup();
                Ok(vec![text(format!("Blurred {n} items: {}.", kinds.join(", ")))])
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
                export::save_doc(&d, &name, &p, &self.settings())?;
                Ok(saved(&p, format!("Saved {}", p.display())))
            }
            "copy_to_clipboard" => Ok(vec![text(self.copy_current()?)]),
            "list_video_encoders" => {
                let list = crate::video::encoders();
                Ok(vec![text(serde_json::to_string_pretty(&list).unwrap_or_default())])
            }
            "record_video" => {
                let secs = num(a, "seconds").ok_or("missing seconds")?.clamp(1.0, 600.0) as u32;
                let s = self.settings();
                let format = match str_of(a, "format") {
                    Some(f) => video_format_arg(f)?,
                    None => s.rec_format.video_or_mp4(),
                };
                let rect = if a.get("width").is_some() {
                    let r = rect_arg(a)?;
                    (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32)
                } else if let Some(id) = num(a, "monitor_id") {
                    let m = capture::monitors().into_iter().find(|m| m.id == id as u32).ok_or("no such monitor (see list_monitors)")?;
                    (m.x, m.y, m.width, m.height)
                } else {
                    capture::monitor_under_cursor()?
                };
                let mut opts = crate::video::VideoOpts::from_settings(&s, rect);
                opts.format = format;
                opts.path = match str_of(a, "path") {
                    Some(p) => PathBuf::from(p),
                    None => crate::video::recording_path(&s.video_folder, "Recording", format.ext()),
                };
                if let Some(f) = num(a, "fps") {
                    opts.fps = f.clamp(1.0, 120.0) as u32;
                }
                if let Some(q) = num(a, "quality") {
                    opts.quality = q.clamp(1.0, 4.0) as u8;
                }
                if let Some(h) = bool_arg(a, "hardware") {
                    opts.hardware = h;
                }
                if let Some(c) = bool_arg(a, "cursor") {
                    opts.cursor = c;
                }
                if let Some(x) = str_of(a, "audio") {
                    opts.audio = crate::audio::AudioSource::parse(x).ok_or("audio is none, system, microphone or both")?;
                }
                if let Some(m) = str_of(a, "microphone") {
                    opts.mic = m.to_string();
                }
                opts.max_s = secs;
                let pause = AtomicBool::new(false);
                let d = self.ticking(Some(secs as f64), "Recording the screen", || crate::video::record(&opts, &self.stop, &pause));
                d.result.clone()?;
                let size = std::fs::metadata(&d.path).map(|m| m.len()).unwrap_or(0);
                Ok(saved(&d.path, format!(
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
                ) + &if d.note.is_empty() { String::new() } else { format!(" {}.", d.note) }))
            }
            "record_gif" => {
                let s = self.settings();
                let rect = if ["x", "y", "width", "height"].iter().any(|k| a.get(*k).is_some()) {
                    let r = rect_arg(a)?;
                    (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32)
                } else {
                    capture::monitor_under_cursor()?
                };
                let secs = num(a, "seconds").unwrap_or(5.0).clamp(1.0, 120.0) as u32;
                let fps = num(a, "fps").map(|f| f.clamp(1.0, 30.0) as u32).unwrap_or(s.gif_fps.clamp(1, 30));
                let max_w = num(a, "max_width").map(|w| w.clamp(16.0, 8192.0) as u32).unwrap_or(s.gif_max_width);
                let cursor = bool_arg(a, "cursor").unwrap_or(s.gif_cursor);
                let path = match str_of(a, "path") {
                    Some(p) => PathBuf::from(p),
                    None => crate::video::recording_path(&s.video_folder, "Recording", "gif"),
                };
                let (tx, rx) = std::sync::mpsc::channel();
                capture::spawn_gif_recorder(rect, fps, secs, max_w, cursor, path, self.stop.clone(), eframe::egui::Context::default(), tx);
                let done = self.ticking(Some(secs as f64), "Recording a GIF", || rx.recv_timeout(Duration::from_secs(secs as u64 + 60))).map_err(|e| e.to_string())?;
                done.result?;
                Ok(saved(&done.path, format!("Recorded {:.1} s ({} frames) to {}", done.seconds, done.frames, done.path.display())))
            }
            "library_search" => {
                let s = self.settings();
                let q = a.get("query").and_then(Value::as_str).unwrap_or("");
                let limit = num(a, "limit").unwrap_or(20.0).max(0.0) as usize;
                let items: Vec<Value> = library::scan(Path::new(&s.library_folder)).into_iter().filter(|i| i.matches(q)).take(limit).map(|i| library_json(&i)).collect();
                Ok(vec![text(serde_json::to_string_pretty(&items).unwrap_or_default())])
            }
            other => self.call_more(other, a),
        }
    }
}

/// The add_mark kind of a mark.
fn kind_name(s: &Shape) -> &'static str {
    match s {
        Shape::Arrow { .. } => "arrow",
        Shape::Line { .. } => "line",
        Shape::Rect { .. } => "box",
        Shape::Ellipse { .. } => "oval",
        Shape::Pen { .. } => "pen",
        Shape::Highlight { .. } => "highlight",
        Shape::Text { .. } => "text",
        Shape::Callout { .. } => "callout",
        Shape::Step { .. } => "step",
        Shape::Stamp { .. } => "stamp",
        Shape::Magnify { .. } => "magnify",
        Shape::Pointer { .. } => "pointer",
        Shape::Spotlight { .. } => "spotlight",
        Shape::Image { .. } => "image",
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
        Shape::Step { n, style, .. } => return format!("step {} at {:.0},{:.0}", style.format(*n), b.center().x, b.center().y),
        Shape::Stamp { emoji, .. } => return format!("stamp {emoji} at {:.0},{:.0}", b.center().x, b.center().y),
        Shape::Magnify { .. } => "magnifier",
        Shape::Pointer { .. } => "pointer",
        Shape::Spotlight { .. } => "spotlight",
        Shape::Image { .. } => "picture piece",
    };
    format!("{kind} at {:.0},{:.0} to {:.0},{:.0}", b.min.x, b.min.y, b.max.x, b.max.y)
}

/// Text lines as plain text and as a list with boxes.
fn text_lines(lines: &[ocr::Line]) -> (String, Vec<Value>) {
    let list = lines
        .iter()
        .map(|l| {
            let r = l.rect();
            json!({"text": l.text, "x": r.min.x.round(), "y": r.min.y.round(), "width": r.width().round(), "height": r.height().round()})
        })
        .collect();
    (ocr::plain_text(lines), list)
}

/// A library picture as JSON.
fn library_json(i: &library::Item) -> Value {
    json!({"path": i.path, "title": i.meta.title, "program": i.meta.app, "kind": i.meta.kind, "created": i.meta.created, "tags": i.meta.tags})
}

/// Text alignment: left, center or right (or 0, 1, 2).
fn align_arg(a: &Value) -> Result<Option<u8>, String> {
    match a.get("align") {
        None => Ok(None),
        Some(v) => match (v.as_str().map(|s| s.trim().to_ascii_lowercase()), v.as_f64()) {
            (Some(s), _) if s == "left" => Ok(Some(0)),
            (Some(s), _) if s == "center" || s == "centre" || s == "middle" => Ok(Some(1)),
            (Some(s), _) if s == "right" => Ok(Some(2)),
            (_, Some(n)) if (0.0..=2.0).contains(&n) => Ok(Some(n as u8)),
            _ => Err("align is left, center or right".into()),
        },
    }
}

fn callout_shape_arg(a: &Value) -> Result<Option<crate::settings::CalloutShape>, String> {
    use crate::settings::CalloutShape;
    match str_of(a, "callout_shape").map(|s| s.trim().to_ascii_lowercase()) {
        None => Ok(None),
        Some(s) => match s.as_str() {
            "rounded" => Ok(Some(CalloutShape::Rounded)),
            "rectangle" | "rect" | "box" => Ok(Some(CalloutShape::Rectangle)),
            "oval" | "ellipse" => Ok(Some(CalloutShape::Oval)),
            _ => Err("callout_shape is rounded, rectangle or oval".into()),
        },
    }
}

fn step_style_arg(a: &Value) -> Result<Option<crate::settings::StepStyle>, String> {
    use crate::settings::StepStyle;
    match str_of(a, "style").map(|s| s.trim().to_string()) {
        None => Ok(None),
        Some(s) => match s.as_str() {
            "numbers" | "number" | "1" => Ok(Some(StepStyle::Numbers)),
            "upper" | "A" => Ok(Some(StepStyle::Upper)),
            "lower" | "a" => Ok(Some(StepStyle::Lower)),
            "roman" | "i" => Ok(Some(StepStyle::Roman)),
            _ => Err("style is numbers, upper, lower or roman".into()),
        },
    }
}

fn heads_arg(a: &Value) -> Result<Option<u8>, String> {
    match num(a, "heads") {
        None => Ok(None),
        Some(h) if [0.0, 1.0, 2.0].contains(&h) => Ok(Some(h as u8)),
        Some(_) => Err("heads is 0, 1 or 2".into()),
    }
}

fn zoom_arg(a: &Value) -> Option<f32> {
    num(a, "zoom").map(|z| z.clamp(1.0, 8.0) as f32)
}

/// Change a mark as update_mark asks; what changed, or why it can't.
fn update_shape(s: &mut Shape, a: &Value) -> Result<Vec<&'static str>, String> {
    let mut what = Vec::new();
    let kind = kind_name(s);
    let no = |prop: &str| format!("a {kind} mark has no {prop}");
    if num(a, "dx").is_some() || num(a, "dy").is_some() {
        s.translate(vec2(num(a, "dx").unwrap_or(0.0) as f32, num(a, "dy").unwrap_or(0.0) as f32));
        what.push("moved");
    }
    if num(a, "x").is_some() || num(a, "y").is_some() {
        let b = s.bounds();
        let to = pos2(num(a, "x").map(|v| v as f32).unwrap_or(b.min.x), num(a, "y").map(|v| v as f32).unwrap_or(b.min.y));
        s.translate(to - b.min);
        what.push("moved");
    }
    if let Some(f) = num(a, "scale") {
        if !(0.05..=20.0).contains(&f) {
            return Err("scale is 0.05-20".into());
        }
        s.scale(f as f32);
        what.push("scaled");
    }
    if let Some(c) = str_of(a, "color") {
        s.set_color(parse_color(Some(c)));
        what.push("colour");
    }
    if let Some(w) = num(a, "width") {
        let e = no("width");
        if !s.set_width(w as f32) {
            return Err(e);
        }
        what.push("width");
    }
    if let Some(f) = bool_arg(a, "fill") {
        let e = no("fill");
        if !s.set_fill(f) {
            return Err(e);
        }
        what.push("fill");
    }
    if let Some(sh) = bool_arg(a, "shadow") {
        let e = no("shadow");
        if !s.set_shadow(sh) {
            return Err(e);
        }
        what.push("shadow");
    }
    if let Some(t) = str_of(a, "text") {
        let e = no("text");
        if !s.set_text(t) {
            return Err(e);
        }
        what.push("text");
    }
    let style_keys = ["font", "bold", "italic", "align", "outline"];
    if style_keys.iter().any(|k| a.get(*k).is_some()) {
        let align = align_arg(a)?;
        let e = no("text style");
        match s {
            Shape::Text { font, bold, italic, outline, align: al, .. } | Shape::Callout { font, bold, italic, outline, align: al, .. } => {
                if let Some(f) = str_of(a, "font") {
                    *font = f.to_string();
                }
                *bold = bool_arg(a, "bold").unwrap_or(*bold);
                *italic = bool_arg(a, "italic").unwrap_or(*italic);
                *outline = bool_arg(a, "outline").unwrap_or(*outline);
                *al = align.unwrap_or(*al);
            }
            _ => return Err(e),
        }
        what.push("text style");
    }
    if let Some(cs) = callout_shape_arg(a)? {
        let e = no("bubble shape");
        let Shape::Callout { shape, .. } = s else { return Err(e) };
        *shape = cs;
        what.push("bubble");
    }
    if let Some(h) = heads_arg(a)? {
        let e = no("arrow heads");
        let Shape::Arrow { heads, .. } = s else { return Err(e) };
        *heads = h;
        what.push("heads");
    }
    if let Some(r) = num(a, "radius") {
        let e = no("corner radius");
        let Shape::Rect { radius, .. } = s else { return Err(e) };
        *radius = r.clamp(0.0, 1000.0) as f32;
        what.push("corners");
    }
    let st = step_style_arg(a)?;
    if num(a, "number").is_some() || st.is_some() {
        let e = no("step number");
        let Shape::Step { n, style, .. } = s else { return Err(e) };
        if let Some(v) = num(a, "number") {
            *n = v.clamp(1.0, 9999.0) as u32;
        }
        *style = st.unwrap_or(*style);
        what.push("step");
    }
    if let Some(z) = zoom_arg(a) {
        let e = no("zoom");
        let Shape::Magnify { zoom, .. } = s else { return Err(e) };
        *zoom = z;
        what.push("zoom");
    }
    what.dedup();
    if what.is_empty() {
        return Err("nothing to change: give dx/dy, x/y, scale, color, width, fill, shadow, text or a style".into());
    }
    Ok(what)
}

fn make_mark(a: &Value, d: &Doc) -> Result<Shape, String> {
    let kind = a.get("kind").and_then(Value::as_str).ok_or("missing kind")?;
    let color = parse_color(a.get("color").and_then(Value::as_str));
    let width = num(a, "width").unwrap_or(6.0).clamp(0.5, 200.0) as f32;
    let shadow = a.get("shadow").and_then(Value::as_bool).unwrap_or(false);
    if kind == "pen" {
        let points = points_arg(a, "points")?.ok_or("pen needs points: [[x, y], ...]")?;
        if points.len() < 2 {
            return Err("pen needs two or more points".into());
        }
        return Ok(Shape::Pen { points, color, width, shadow });
    }
    let p1 = pos2(need(a, "x")? as f32, need(a, "y")? as f32);
    let p2 = match (num(a, "x2"), num(a, "y2")) {
        (Some(x), Some(y)) => Some(pos2(x as f32, y as f32)),
        _ => None,
    };
    let need2 = || p2.ok_or_else(|| format!("{kind} needs x2 and y2"));
    let size = num(a, "size").map(|s| s.clamp(1.0, 2000.0) as f32);
    let fill = a.get("fill").and_then(Value::as_bool).unwrap_or(false);
    let txt = a.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    let font = str_of(a, "font").unwrap_or("").to_string();
    let bold = bool_arg(a, "bold").unwrap_or(false);
    let italic = bool_arg(a, "italic").unwrap_or(false);
    let align = align_arg(a)?.unwrap_or(0);
    Ok(match kind {
        "arrow" => Shape::Arrow { a: p1, b: need2()?, color, width, shadow, heads: heads_arg(a)?.unwrap_or(1) },
        "line" => Shape::Line { a: p1, b: need2()?, color, width, shadow },
        "box" => Shape::Rect { rect: Rect::from_two_pos(p1, need2()?), color, width, fill, radius: num(a, "radius").unwrap_or(0.0).clamp(0.0, 1000.0) as f32, shadow },
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
            font,
            bold,
            italic,
            outline: bool_arg(a, "outline").unwrap_or(true),
            align,
        },
        "callout" => Shape::Callout {
            pos: p1,
            tail: p2.unwrap_or(p1 + vec2(-40.0, 60.0)),
            text: txt,
            color,
            size: size.unwrap_or(doc::text_size(width)),
            shadow,
            font,
            bold,
            italic,
            shape: callout_shape_arg(a)?.unwrap_or_default(),
            outline: bool_arg(a, "outline").unwrap_or(false),
            align,
        },
        "step" => Shape::Step {
            center: p1,
            n: num(a, "number").map(|n| n.clamp(1.0, 9999.0) as u32).unwrap_or_else(|| d.next_step(1)),
            color,
            radius: size.unwrap_or(doc::step_radius(width)),
            shadow,
            style: step_style_arg(a)?.unwrap_or_default(),
        },
        "stamp" => Shape::Stamp {
            center: p1,
            emoji: if txt.is_empty() { "\u{2705}".into() } else { txt },
            size: size.unwrap_or(doc::text_size(width) * doc::STAMP_FACTOR),
            color,
        },
        "pointer" => Shape::Pointer { pos: p1, size: size.unwrap_or(doc::pointer_size(width)) },
        "magnify" => Shape::Magnify { center: p1, radius: size.unwrap_or(80.0), zoom: zoom_arg(a).unwrap_or(2.0) },
        "image" => {
            let path = str_of(a, "path").ok_or("kind image needs a path")?;
            let img = image::open(path).map_err(|e| format!("{path}: {e}"))?.to_rgba8();
            let rect = match p2 {
                Some(b) => Rect::from_two_pos(p1, b),
                None => Rect::from_min_size(p1, vec2(img.width() as f32, img.height() as f32)),
            };
            if rect.width() < 1.0 || rect.height() < 1.0 {
                return Err("the image needs some size".into());
            }
            Shape::Image { rect, image: doc::ImgData(Arc::new(img)) }
        }
        other => return Err(format!("unknown kind {other}")),
    })
}

fn result(id: &Value, result: Value) -> Option<String> {
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
}

fn error(id: &Value, code: i64, message: &str) -> Option<String> {
    Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string())
}

/// Which tool calls are running or waiting, so a cancel stops only the call it names.
#[derive(Default)]
struct Calls {
    /// The request id (as JSON text) of the call running now.
    running: Option<String>,
    /// Calls waiting for the running one to finish.
    waiting: Vec<String>,
    /// Waiting calls that were cancelled: they won't run.
    cancelled: Vec<String>,
}

/// One AI app's connection: its current picture, and a flag that stops a running recording when
/// the app cancels the call (reachable without waiting for the running tool).
pub struct Session {
    ws: Mutex<Workspace>,
    stop: Arc<AtomicBool>,
    calls: Mutex<Calls>,
    /// The protocol version agreed at initialize.
    version: Mutex<String>,
    /// Settings used instead of the saved file (tests).
    fixed: Option<Settings>,
}

impl Default for Session {
    fn default() -> Self {
        Session::from_workspace(Workspace::default())
    }
}

const PROMPTS: [(&str, &str, &str, &str); 5] = [
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
    (
        "change_language",
        "Switch Snagster's interface to another language.",
        "code",
        "Set Snagster's interface language to {code} with set_language (list_languages has the codes), then tell me the language's own name.",
    ),
];

const PROMPT_ARG_HELP: [(&str, &str); 5] = [
    ("what", "What to point out."),
    ("window", "Part of the window's title."),
    ("seconds", "How long to record."),
    ("topic", "What the guide is about."),
    ("code", "A language code such as de, ja or pt-BR (or auto)."),
];

fn prompts_list() -> Value {
    let list: Vec<Value> = PROMPTS
        .iter()
        .map(|(name, desc, arg, _)| {
            let help = PROMPT_ARG_HELP.iter().find(|h| h.0 == *arg).map(|h| h.1).unwrap_or_default();
            json!({"name": name, "description": desc, "arguments": [{"name": arg, "description": help, "required": false}]})
        })
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
        "code" => "auto",
        _ => "this task",
    });
    Ok(json!({"description": desc, "messages": [{"role": "user", "content": {"type": "text", "text": body.replace(&format!("{{{arg}}}"), value)}}]}))
}

const TEMPLATES: [(&str, &str, &str, &str); 3] = [
    ("snagster://library/{path}", "library-picture", "A picture in the library, by the path library_search gives", "image/png"),
    ("snagster://settings/{key}", "setting", "One of Snagster's settings, by name", "application/json"),
    ("snagster://languages/{code}", "language", "One of the interface languages, by code", "application/json"),
];

/// Completions for a prompt argument or a resource template's variable: what starts with
/// `value` (case ignored), at most 100.
fn completions(settings: &Settings, r: &Value, arg: &str, value: &str) -> Value {
    let all: Vec<String> = match (r.get("type").and_then(Value::as_str), arg) {
        (_, "code") => std::iter::once("auto").chain(crate::i18n::CODES.iter().copied()).map(String::from).collect(),
        (Some("ref/prompt"), "seconds") => ["5", "10", "30", "60", "120"].map(String::from).to_vec(),
        (Some("ref/prompt"), "what") => ["the most important button", "the menu", "the error message", "the search box"].map(String::from).to_vec(),
        (Some("ref/prompt"), "window") => capture::list_windows().into_iter().map(|w| w.label).filter(|l| !l.is_empty()).collect(),
        (Some("ref/resource"), "key") => serde_json::to_value(settings).ok().and_then(|v| v.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default(),
        (Some("ref/resource"), "path") => library::scan(Path::new(&settings.library_folder)).into_iter().map(|i| i.path.to_string_lossy().to_string()).collect(),
        _ => Vec::new(),
    };
    let low = value.to_lowercase();
    let mut found: Vec<String> = all.into_iter().filter(|v| v.to_lowercase().starts_with(&low)).collect();
    found.dedup();
    let total = found.len();
    found.truncate(100);
    json!({"completion": {"values": found, "total": total, "hasMore": total > 100}})
}

impl Session {
    fn from_workspace(ws: Workspace) -> Session {
        let stop = ws.stop.clone();
        let fixed = ws.fixed.clone();
        Session { ws: Mutex::new(ws), stop, calls: Mutex::new(Calls::default()), version: Mutex::new(PROTOCOL_VERSIONS[0].to_string()), fixed }
    }

    /// A session that uses (and changes) these settings instead of the saved ones.
    #[cfg(test)]
    pub fn with_settings(s: Settings) -> Session {
        Session::from_workspace(Workspace::with_settings(s))
    }

    fn settings(&self) -> Settings {
        self.fixed.clone().unwrap_or_else(|| Settings::load(false))
    }

    fn calls(&self) -> MutexGuard<'_, Calls> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `notifications/cancelled`: stop the named call if it runs, or drop it if it waits.
    fn cancel(&self, params: &Value) {
        let Some(id) = params.get("requestId") else { return };
        let key = id.to_string();
        let mut c = self.calls();
        if c.running.as_deref() == Some(key.as_str()) {
            self.stop.store(true, Ordering::Relaxed);
        } else if c.waiting.contains(&key) && !c.cancelled.contains(&key) {
            c.cancelled.push(key);
        }
    }

    /// Read one resource (or a template's).
    fn read_resource(&self, uri: &str) -> Result<Value, (i64, String)> {
        let json_text = |v: &Value| json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]});
        let missing = || (-32002, format!("Resource not found: {uri}"));
        match uri {
            "snagster://settings" => return Ok(json_text(&serde_json::to_value(self.settings()).unwrap_or_default())),
            "snagster://library" => {
                let s = self.settings();
                let items: Vec<Value> = library::scan(Path::new(&s.library_folder)).iter().take(50).map(library_json).collect();
                return Ok(json_text(&json!(items)));
            }
            "snagster://picture/current" => {
                let png = self.ws.lock().ok().and_then(|ws| ws.current_png());
                return match png {
                    Some((_, img)) => Ok(json!({"contents": [{"uri": uri, "mimeType": "image/png", "blob": img.get("data").cloned().unwrap_or_default()}]})),
                    None => Err((-32002, "There is no current picture yet".into())),
                };
            }
            _ => {}
        }
        if let Some(key) = uri.strip_prefix("snagster://settings/") {
            let key = crate::mcp_http::percent_decode(key);
            let v = serde_json::to_value(self.settings()).unwrap_or_default();
            return v.get(&key).map(json_text).ok_or_else(missing);
        }
        if let Some(code) = uri.strip_prefix("snagster://languages/") {
            let code = crate::mcp_http::percent_decode(code);
            let c = crate::i18n::CODES.iter().find(|c| c.eq_ignore_ascii_case(&code)).ok_or_else(missing)?;
            return Ok(json_text(&json!({"code": c, "name": crate::i18n::name_in(c, "en"), "native": crate::i18n::native_name(c)})));
        }
        if let Some(p) = uri.strip_prefix("snagster://library/") {
            let want = PathBuf::from(crate::mcp_http::percent_decode(p));
            let s = self.settings();
            let item = library::scan(Path::new(&s.library_folder)).into_iter().find(|i| more::same_file(&i.path, &want)).ok_or_else(missing)?;
            let bytes = std::fs::read(&item.path).map_err(|e| (-32603, e.to_string()))?;
            return Ok(json!({"contents": [{"uri": uri, "mimeType": "image/png", "blob": base64::engine::general_purpose::STANDARD.encode(bytes)}]}));
        }
        Err(missing())
    }

    /// Handle one JSON-RPC message; the reply line, or None for notifications and replies.
    pub fn handle(&self, line: &str) -> Option<String> {
        self.handle_with(line, None)
    }

    /// Handle one message; `notify` carries progress notifications while a tool runs (when the
    /// request asked for them with `_meta.progressToken`).
    pub fn handle_with(&self, line: &str, notify: Option<Notify>) -> Option<String> {
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
                // A notification. Cancelling stops the named recording or scroll.
                if method == "notifications/cancelled" {
                    self.cancel(&params);
                }
                return None;
            }
        };
        match method.as_str() {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
                if let Ok(mut v) = self.version.lock() {
                    *v = version.to_string();
                }
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
            "resources/templates/list" => {
                let list: Vec<Value> = TEMPLATES.iter().map(|(t, name, title, mime)| json!({"uriTemplate": t, "name": name, "title": title, "mimeType": mime})).collect();
                result(&id, json!({ "resourceTemplates": list }))
            }
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or_default();
                match self.read_resource(uri) {
                    Ok(v) => result(&id, v),
                    Err((c, m)) => error(&id, c, &m),
                }
            }
            "completion/complete" => {
                let r = params.get("ref").cloned().unwrap_or(Value::Null);
                let arg = params.pointer("/argument/name").and_then(Value::as_str).unwrap_or_default();
                let value = params.pointer("/argument/value").and_then(Value::as_str).unwrap_or_default();
                result(&id, completions(&self.settings(), &r, arg, value))
            }
            "logging/setLevel" => result(&id, json!({})),
            "tools/call" => self.call_tool(&id, &params, notify),
            _ => error(&id, -32601, &format!("Method not found: {method}")),
        }
    }

    /// Run a `tools/call`: one at a time, cancellable by its id, with progress when asked.
    fn call_tool(&self, id: &Value, params: &Value, notify: Option<Notify>) -> Option<String> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let key = id.to_string();
        self.calls().waiting.push(key.clone());
        let mut ws = match self.ws.lock() {
            Ok(ws) => ws,
            Err(p) => p.into_inner(),
        };
        {
            let mut c = self.calls();
            if let Some(i) = c.waiting.iter().position(|k| *k == key) {
                c.waiting.remove(i);
            }
            // Cancelled while it waited: it never runs, and gets no answer.
            if let Some(i) = c.cancelled.iter().position(|k| *k == key) {
                c.cancelled.remove(i);
                return None;
            }
            self.stop.store(false, Ordering::Relaxed);
            c.running = Some(key);
        }
        ws.progress = match (params.pointer("/_meta/progressToken").cloned(), notify) {
            (Some(token), Some(n)) if !token.is_null() => Some(Arc::new(move |done: f64, total: Option<f64>, what: &str| n(progress_note(&token, done, total, what))) as Progress),
            _ => None,
        };
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ws.call(&name, &args))).unwrap_or_else(|_| Err("the tool stopped unexpectedly (Snagster kept running)".into()));
        ws.progress = None;
        let structured = ws.structured.take();
        drop(ws);
        self.calls().running = None;
        let links = self.version.lock().map(|v| v.as_str() >= LINKS_SINCE).unwrap_or(true);
        match out {
            Ok(mut content) => {
                // A JSON answer also goes out as structured content (lists inside {"items": ...}).
                let structured = structured.or_else(|| {
                    let only = content.first().filter(|_| content.len() == 1)?;
                    match serde_json::from_str::<Value>(only.get("text")?.as_str()?).ok()? {
                        v @ Value::Object(_) => Some(v),
                        v @ Value::Array(_) => Some(json!({ "items": v })),
                        _ => None,
                    }
                });
                if !links {
                    content.retain(|c| c.get("type").and_then(Value::as_str) != Some("resource_link"));
                }
                match structured {
                    Some(s) => result(id, json!({"content": content, "structuredContent": s, "isError": false})),
                    None => result(id, json!({"content": content, "isError": false})),
                }
            }
            Err(e) => result(id, json!({"content": [text(e)], "isError": true})),
        }
    }
}

/// Serve MCP over stdin/stdout until stdin closes. Tool calls run on their own thread, so a
/// cancel (or a ping) is answered while a long recording runs; progress goes out as it comes.
pub fn serve_stdio() {
    let session = Arc::new(Session::default());
    let out = Arc::new(Mutex::new(std::io::stdout()));
    fn send(out: &Mutex<std::io::Stdout>, line: &str) {
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
                let o2 = o.clone();
                let notify: Notify = Arc::new(move |msg: String| send(&o2, &msg));
                if let Some(reply) = s.handle_with(&line, Some(notify)) {
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
                "Snagster {}\n\nUse: snagster [command]\n\n  (no command)                          open the window\n  --mcp                                 MCP server for AI apps (stdin/stdout)\n  capture screen [-o out.png]           the screen under the mouse\n  capture all [-o out.png]              all screens\n  capture region X Y W H [-o out.png]   a desktop rectangle\n  capture window TITLE [-o out.png]     a window whose title contains TITLE\n  ocr IMAGE                             print the text in a picture\n  redact IMAGE -o OUT                   blur emails, phone numbers... and save\n  effect IMAGE EFFECT -o OUT            apply an effect ({})\n  convert IMAGE -o OUT                  save in another format (by extension)\n  record [SECONDS] [-o out.mp4]         record the screen to a video (default 10 s)\n      [--fps 1-120] [--format mp4|hevc|wmv] [--quality 1-4]\n      [--region X Y W H] [--software] [--no-cursor]\n  encoders                              list the video encoders\n  --hw-report [FILE]                    test graphics-chip encoding here, write a report\n  windows | monitors                    list windows or screens\n  tools                                 list the AI tools\n  call TOOL [JSON]                      run one AI tool, e.g. call list_windows\n",
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

    // ---- helpers ----

    /// A fresh folder in the temp directory for one test.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("snagster-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A workspace holding a plain `w` x `h` picture (no file, no screen), with default settings.
    fn ws_with(w: u32, h: u32) -> Workspace {
        let mut ws = Workspace::with_settings(Settings::default());
        ws.set(Doc::new(image::RgbaImage::from_pixel(w, h, image::Rgba([200, 210, 220, 255]))), "test").unwrap();
        ws
    }

    fn err_of(o: Outcome) -> String {
        o.expect_err("the tool should have refused")
    }

    fn text_of(o: Outcome) -> String {
        let items = o.expect("the tool should have worked");
        items[0]["text"].as_str().unwrap_or_default().to_string()
    }

    fn rpc(id: Value, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    // ---- JSON-RPC ----

    #[test]
    fn initialize_reports_capabilities_server_and_instructions() {
        let s = Session::default();
        let r = ask(&s, rpc(json!(1), "initialize", json!({})))["result"].clone();
        assert_eq!(r["protocolVersion"], PROTOCOL_VERSIONS[0], "no version asked: the newest");
        assert_eq!(r["serverInfo"]["name"], "snagster");
        assert_eq!(r["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(r["instructions"], INSTRUCTIONS);
        for cap in ["tools", "resources", "prompts", "completions", "logging"] {
            assert!(r["capabilities"][cap].is_object(), "capability {cap}");
        }
        for v in PROTOCOL_VERSIONS {
            let r = ask(&s, rpc(json!(2), "initialize", json!({"protocolVersion": v})));
            assert_eq!(r["result"]["protocolVersion"], v, "a supported version is echoed");
        }
    }

    #[test]
    fn replies_carry_the_request_id_and_jsonrpc_version() {
        let s = Session::default();
        for id in [json!(7), json!("abc-1"), json!(0)] {
            let r = ask(&s, rpc(id.clone(), "ping", json!({})));
            assert_eq!(r["id"], id);
            assert_eq!(r["jsonrpc"], "2.0");
        }
        // An error for an unknown method keeps the id too.
        let r = ask(&s, rpc(json!("x"), "tools/nothing", json!({})));
        assert_eq!(r["id"], "x");
        assert_eq!(r["error"]["code"], -32601);
        assert!(r["error"]["message"].as_str().unwrap().contains("tools/nothing"));
    }

    #[test]
    fn requests_without_a_method_are_invalid_and_replies_are_ignored() {
        let s = Session::default();
        let r = ask(&s, json!({"jsonrpc": "2.0", "id": 9}));
        assert_eq!(r["error"]["code"], -32600);
        assert_eq!(r["id"], 9);
        // A client's answer to something (result or error) needs no reply.
        assert!(s.handle(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#).is_none());
        assert!(s.handle(r#"{"jsonrpc":"2.0","id":3,"error":{"code":1,"message":"x"}}"#).is_none());
        // Parse errors have a null id.
        let r: Value = serde_json::from_str(&s.handle("{\"jsonrpc\":").unwrap()).unwrap();
        assert_eq!(r["error"]["code"], -32700);
        assert!(r["id"].is_null());
        // Batches are refused with a null id.
        let r: Value = serde_json::from_str(&s.handle("[]").unwrap()).unwrap();
        assert_eq!(r["error"]["code"], -32600);
    }

    /// Bug: any cancel stopped whatever ran, even when it named another (finished) request.
    #[test]
    fn a_cancel_stops_only_the_call_it_names() {
        let s = Session::default();
        let cancel = |id: Value| s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id}}).to_string());
        assert!(s.handle(r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{}}"#).is_none());
        assert!(cancel(json!(1)).is_none(), "no reply to a notification");
        assert!(!s.stop.load(Ordering::Relaxed), "nothing runs: nothing stops");
        s.calls().running = Some(json!(5).to_string());
        cancel(json!(4));
        assert!(!s.stop.load(Ordering::Relaxed), "another request");
        cancel(json!("5"));
        assert!(!s.stop.load(Ordering::Relaxed), "the string \"5\" is not the number 5");
        cancel(json!(5));
        assert!(s.stop.load(Ordering::Relaxed));
        // The workspace sees the same flag.
        assert!(s.ws.lock().unwrap().stop.load(Ordering::Relaxed));
    }

    #[test]
    fn a_waiting_call_that_is_cancelled_never_runs() {
        let s = Session::with_settings(Settings::default());
        s.calls().waiting.push(json!(7).to_string());
        s.handle(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#);
        assert_eq!(s.calls().cancelled, [json!(7).to_string()]);
        let r = s.handle(&rpc(json!(7), "tools/call", json!({"name": "list_languages"})).to_string());
        assert!(r.is_none(), "a cancelled request gets no answer");
        assert!(s.calls().cancelled.is_empty() && s.calls().running.is_none());
        // The next call with that id runs as usual.
        assert_eq!(call(&s, 7, "list_languages", json!({}))["isError"], false);
    }

    #[test]
    fn a_running_capture_delay_is_cancelled_by_its_id() {
        let s = Arc::new(Session::with_settings(Settings::default()));
        let s2 = s.clone();
        let t0 = Instant::now();
        let worker = std::thread::spawn(move || call(&s2, 9, "capture_screen", json!({"delay_s": 30})));
        while s.calls().running.is_none() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        s.handle(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":8}}"#);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!worker.is_finished(), "cancelling request 8 leaves request 9 running");
        s.handle(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9}}"#);
        let r = worker.join().unwrap();
        assert_eq!(r["isError"], true);
        assert_eq!(r["content"][0]["text"], "cancelled");
        assert!(t0.elapsed() < Duration::from_secs(20), "it stopped waiting");
        // A new call starts with the stop flag down again.
        assert_eq!(call(&s, 10, "list_languages", json!({}))["isError"], false);
        assert!(!s.stop.load(Ordering::Relaxed));
    }

    #[test]
    fn progress_notifications_follow_the_token() {
        let dir = temp_dir("progress");
        let a = dir.join("a.png");
        image::RgbaImage::from_pixel(20, 10, image::Rgba([1, 2, 3, 255])).save(&a).unwrap();
        let s = Session::with_settings(Settings::default());
        let got = Arc::new(Mutex::new(Vec::<String>::new()));
        let g = got.clone();
        let notify: Notify = Arc::new(move |m: String| g.lock().unwrap().push(m));
        let msg = |meta: Value| rpc(json!(1), "tools/call", json!({"name": "make_guide", "arguments": {"path": dir.join("g.html"), "steps": [{"path": a}, {"path": a}]}, "_meta": meta})).to_string();
        // No token: no progress.
        let r: Value = serde_json::from_str(&s.handle_with(&msg(json!({})), Some(notify.clone())).unwrap()).unwrap();
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert!(got.lock().unwrap().is_empty());
        s.handle_with(&msg(json!({"progressToken": "tok-1"})), Some(notify)).unwrap();
        let notes: Vec<Value> = got.lock().unwrap().iter().map(|m| serde_json::from_str(m).unwrap()).collect();
        assert_eq!(notes.len(), 3, "two steps read, then saved: {notes:?}");
        let mut last = 0.0;
        for n in &notes {
            assert_eq!(n["jsonrpc"], "2.0");
            assert!(n.get("id").is_none(), "a notification");
            assert_eq!(n["method"], "notifications/progress");
            assert_eq!(n["params"]["progressToken"], "tok-1");
            assert_eq!(n["params"]["total"], 3.0);
            let p = n["params"]["progress"].as_f64().unwrap();
            assert!(p > last, "progress goes up");
            last = p;
            assert!(n["params"]["message"].is_string());
        }
        assert_eq!(last, 3.0);
        assert!(s.ws.lock().unwrap().progress.is_none(), "the reporter goes away with the call");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_note_format() {
        let n: Value = serde_json::from_str(&progress_note(&json!(42), 1.5, Some(10.0), "Recording")).unwrap();
        assert_eq!(n, json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": 42, "progress": 1.5, "total": 10.0, "message": "Recording"}}));
        let n: Value = serde_json::from_str(&progress_note(&json!("t"), 2.0, None, "x")).unwrap();
        assert!(n["params"].get("total").is_none(), "no total when it isn't known");
    }

    #[test]
    fn ticking_reports_rising_seconds_up_to_the_total() {
        let mut ws = Workspace::default();
        let got = Arc::new(Mutex::new(Vec::<(f64, Option<f64>)>::new()));
        let g = got.clone();
        ws.progress = Some(Arc::new(move |d: f64, t: Option<f64>, _: &str| g.lock().unwrap().push((d, t))));
        let r = ws.ticking_every(Duration::from_millis(30), Some(0.2), "x", || {
            std::thread::sleep(Duration::from_millis(400));
            7
        });
        assert_eq!(r, 7, "the work's answer comes back");
        let got = got.lock().unwrap();
        assert_eq!(got[0], (0.0, Some(0.2)), "a first report at once");
        assert!(got.len() >= 2, "{got:?}");
        assert!(got.windows(2).all(|w| w[1].0 > w[0].0), "always rising: {got:?}");
        assert!(got.iter().all(|p| p.0 <= 0.2), "never past the total: {got:?}");
        // Without a reporter the work just runs.
        assert_eq!(Workspace::default().ticking(None, "x", || 3), 3);
    }

    #[test]
    fn small_methods_answer() {
        let s = Session::default();
        assert_eq!(ask(&s, rpc(json!(1), "logging/setLevel", json!({"level": "info"})))["result"], json!({}));
        let t = ask(&s, rpc(json!(2), "resources/templates/list", json!({})))["result"]["resourceTemplates"].clone();
        let uris: Vec<&str> = t.as_array().unwrap().iter().map(|r| r["uriTemplate"].as_str().unwrap()).collect();
        assert_eq!(uris, ["snagster://library/{path}", "snagster://settings/{key}", "snagster://languages/{code}"]);
        assert!(t.as_array().unwrap().iter().all(|r| r["name"].is_string() && r["mimeType"].is_string()));
        let c = ask(&s, rpc(json!(3), "completion/complete", json!({})))["result"]["completion"].clone();
        assert_eq!(c["values"], json!([]));
        assert_eq!(c["hasMore"], false);
    }

    #[test]
    fn completions_offer_language_codes_prompt_values_and_setting_names() {
        let s = Session::with_settings(Settings::default());
        let complete = |r: Value, name: &str, value: &str| ask(&s, rpc(json!(1), "completion/complete", json!({"ref": r, "argument": {"name": name, "value": value}})))["result"]["completion"].clone();
        let prompt = json!({"type": "ref/prompt", "name": "change_language"});
        let c = complete(prompt.clone(), "code", "pt");
        let values: Vec<&str> = c["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(values.contains(&"pt-BR"), "{values:?}");
        assert!(values.iter().all(|v| v.to_lowercase().starts_with("pt")));
        assert_eq!(c["total"], values.len());
        let all = complete(prompt, "code", "");
        assert_eq!(all["total"], crate::i18n::CODES.len() + 1, "every code and auto");
        assert_eq!(all["hasMore"], false, "86 + 1 fit in one answer");
        let secs = complete(json!({"type": "ref/prompt", "name": "record_demo"}), "seconds", "1");
        assert_eq!(secs["values"], json!(["10", "120"]));
        let keys = complete(json!({"type": "ref/resource", "uri": "snagster://settings/{key}"}), "key", "VIDEO_F");
        let keys: Vec<&str> = keys["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(keys.contains(&"video_fps") && keys.contains(&"video_folder"), "{keys:?}");
        assert_eq!(complete(json!({"type": "ref/prompt", "name": "x"}), "unknown", "")["values"], json!([]));
    }

    #[test]
    fn template_resources_read_settings_languages_and_library_pictures() {
        let dir = temp_dir("templates");
        let lib = dir.join("lib");
        let s = Settings { library_folder: lib.to_string_lossy().to_string(), video_fps: 48, ..Default::default() };
        let pic = library::add(&lib, &image::RgbaImage::from_pixel(6, 4, image::Rgba([9, 8, 7, 255])), library::Meta { title: "One".into(), ..Default::default() }).unwrap();
        let session = Session::with_settings(s);
        let read = |uri: String| ask(&session, rpc(json!(1), "resources/read", json!({ "uri": uri })));
        let r = read("snagster://settings/video_fps".into());
        assert_eq!(r["result"]["contents"][0]["text"], "48");
        assert_eq!(read("snagster://settings/nope".into())["error"]["code"], -32002);
        let r = read("snagster://languages/DE".into());
        let v: Value = serde_json::from_str(r["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["code"], "de");
        assert_eq!(v["native"], crate::i18n::native_name("de"));
        assert_eq!(read("snagster://languages/xx".into())["error"]["code"], -32002);
        // A library picture, by its (encoded) path.
        let enc: String = pic.to_string_lossy().bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        let r = read(format!("snagster://library/{enc}"));
        assert_eq!(r["result"]["contents"][0]["mimeType"], "image/png", "{r}");
        let png = base64::engine::general_purpose::STANDARD.decode(r["result"]["contents"][0]["blob"].as_str().unwrap()).unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().width(), 6);
        // Files outside the library are not served.
        let outside = dir.join("secret.png");
        image::RgbaImage::new(2, 2).save(&outside).unwrap();
        assert_eq!(read(format!("snagster://library/{}", outside.display()))["error"]["code"], -32002);
        let r = read("snagster://library".into());
        assert!(r["result"]["contents"][0]["text"].as_str().unwrap().contains("One"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_come_back_as_structured_items_matching_their_output_schema() {
        let s = Session::with_settings(Settings { library_folder: temp_dir("lists").join("lib").to_string_lossy().to_string(), ..Default::default() });
        let list = tool_list();
        for name in ["list_languages", "list_presets", "list_video_encoders", "library_search"] {
            let schema = list["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap()["outputSchema"].clone();
            assert_eq!(schema["type"], "object", "{name}");
            assert_eq!(schema["required"], json!(["items"]), "{name}");
            let r = call(&s, 1, name, json!({}));
            assert_eq!(r["isError"], false, "{name}: {r}");
            let items = &r["structuredContent"]["items"];
            assert!(items.is_array(), "{name}: {r}");
            // The text still holds the plain list for older apps.
            let text_list: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(&text_list, items, "{name}");
        }
        let langs = call(&s, 2, "list_languages", json!({}));
        assert_eq!(langs["structuredContent"]["items"].as_array().unwrap().len(), crate::i18n::CODES.len());
        let presets = call(&s, 3, "list_presets", json!({}));
        assert_eq!(presets["structuredContent"]["items"][0]["mode"], "Area");
        // Every tool with an output schema is a known tool, and the schema is an object schema.
        let names: Vec<&str> = list["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        for t in list["tools"].as_array().unwrap() {
            if let Some(o) = t.get("outputSchema") {
                assert_eq!(o["type"], "object", "{}", t["name"]);
                assert!(o["properties"].is_object(), "{}", t["name"]);
            }
        }
        assert!(names.contains(&"read_text") && output_schema("read_text").is_some());
        assert!(output_schema("crop").is_none());
    }

    #[test]
    fn marks_are_listed_as_structured_items() {
        let s = Session::with_settings(Settings::default());
        *s.ws.lock().unwrap() = ws_with(100, 100);
        call(&s, 1, "add_mark", json!({"kind": "box", "x": 10, "y": 20, "x2": 30, "y2": 60, "width": 2}));
        let r = call(&s, 2, "list_marks", json!({}));
        let item = &r["structuredContent"]["items"][0];
        assert_eq!(item["index"], 0);
        assert_eq!(item["kind"], "box");
        assert_eq!((item["x"].as_f64(), item["width"].as_f64()), (Some(9.0), Some(22.0)), "the box with its line");
        assert!(r["content"][0]["text"].as_str().unwrap().starts_with("0: box"));
    }

    #[test]
    fn saved_files_come_with_a_resource_link_for_newer_apps() {
        let dir = temp_dir("links");
        let s = Session::with_settings(Settings::default());
        *s.ws.lock().unwrap() = ws_with(30, 20);
        let out = dir.join("a b.png");
        let r = call(&s, 1, "save", json!({"path": out}));
        assert_eq!(r["content"][0]["type"], "text");
        let l = &r["content"][1];
        assert_eq!(l["type"], "resource_link");
        assert_eq!(l["name"], "a b.png");
        assert_eq!(l["mimeType"], "image/png");
        assert!(l["uri"].as_str().unwrap().starts_with("file:///") && l["uri"].as_str().unwrap().ends_with("/a%20b.png"), "{l}");
        assert!(l["size"].as_u64().unwrap() > 0);
        // An app on an older protocol gets only the text.
        ask(&s, rpc(json!(2), "initialize", json!({"protocolVersion": "2025-03-26"})));
        let r = call(&s, 3, "save", json!({"path": out}));
        assert_eq!(r["content"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_addresses_are_absolute_and_escaped() {
        #[cfg(windows)]
        assert_eq!(file_uri(Path::new(r"C:\Users\A B\pic #1.png")), "file:///C:/Users/A%20B/pic%20%231.png");
        #[cfg(not(windows))]
        assert_eq!(file_uri(Path::new("/tmp/A B/pic #1.png")), "file:///tmp/A%20B/pic%20%231.png");
        let rel = file_uri(Path::new("x.png"));
        assert!(rel.starts_with("file:///") && rel.ends_with("/x.png"), "made absolute: {rel}");
        assert!(!rel.contains('\\'));
    }

    // ---- tools/list and schemas ----

    #[test]
    fn every_schema_is_a_valid_object_schema() {
        let list = tool_list();
        let allowed = ["string", "number", "integer", "boolean", "object", "array"];
        for t in list["tools"].as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{name} is snake_case");
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object", "{name}");
            let props = schema["properties"].as_object().unwrap_or_else(|| panic!("{name} has properties"));
            let required = schema["required"].as_array().unwrap_or_else(|| panic!("{name} has a required list"));
            for r in required {
                let r = r.as_str().expect("required names are strings");
                assert!(props.contains_key(r), "{name} requires {r}, which isn't a property");
            }
            for (p, def) in props {
                assert!(def.is_object(), "{name}.{p} is a schema object");
                if let Some(ty) = def.get("type") {
                    assert!(allowed.contains(&ty.as_str().unwrap()), "{name}.{p} type {ty}");
                    if ty == "array" {
                        assert!(def["items"].is_object(), "{name}.{p} says what its items are");
                    }
                }
                if let Some(e) = def.get("enum") {
                    let e = e.as_array().unwrap();
                    assert!(!e.is_empty() && e.iter().all(Value::is_string), "{name}.{p} enum");
                }
            }
        }
    }

    #[test]
    fn titles_and_hints_follow_the_tools() {
        let list = tool_list();
        let tools = list["tools"].as_array().unwrap();
        let find = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("tool {n}")).clone();
        let t = find("list_marks");
        assert_eq!(t["title"], "List marks");
        assert_eq!(t["annotations"]["title"], "List marks");
        assert_eq!(t["annotations"]["readOnlyHint"], true);
        assert_eq!(t["annotations"]["idempotentHint"], true);
        for side in ["save", "copy_to_clipboard", "record_video", "set_setting", "capture_screen", "studio_add_source", "add_mark", "crop"] {
            assert_eq!(find(side)["annotations"]["readOnlyHint"], false, "{side} changes something");
        }
        for d in ["remove_mark", "clear_marks", "studio_remove_source", "studio_remove_scene", "library_delete", "set_setting", "save", "make_guide"] {
            assert_eq!(find(d)["annotations"]["destructiveHint"], true, "{d}");
        }
        for side in ["quick_save", "library_tag", "library_delete", "studio_add_scene", "studio_set_current", "studio_update_source", "update_mark", "run_preset"] {
            assert_eq!(find(side)["annotations"]["readOnlyHint"], false, "{side} changes something");
        }
        assert_eq!(find("add_mark")["annotations"]["destructiveHint"], false);
        assert_eq!(find("quick_save")["annotations"]["destructiveHint"], false, "it always makes a new file");
        // Doing it twice is the same as once...
        for i in ["save", "set_setting", "set_language", "studio_set_current", "library_tag", "list_marks"] {
            assert_eq!(find(i)["annotations"]["idempotentHint"], true, "{i}");
        }
        // ...but not for these.
        for i in ["add_mark", "capture_screen", "record_video", "quick_save", "undo", "studio_add_scene", "update_mark"] {
            assert_eq!(find(i)["annotations"]["idempotentHint"], false, "{i}");
        }
        // Only tools that exist are named in the hint lists.
        for n in SIDE.iter().chain(DESTRUCTIVE.iter()).chain(IDEMPOTENT.iter()) {
            find(n);
        }
        for t in tools {
            let open = t["name"] == "check_for_update" || t["name"] == "stream_start";
            assert_eq!(t["annotations"]["openWorldHint"], open, "{}", t["name"]);
            assert!(t["title"].as_str().unwrap().chars().next().unwrap().is_uppercase());
        }
    }

    #[test]
    fn tool_names_match_the_list_and_effects_are_offered() {
        let list = tool_list();
        let listed: Vec<&str> = list["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        let names: Vec<&str> = tool_names().into_iter().map(|(n, _)| n).collect();
        assert_eq!(listed, names);
        let s = Session::default();
        assert_eq!(ask(&s, rpc(json!(1), "tools/list", json!({})))["result"], list);
        let effect = list["tools"].as_array().unwrap().iter().find(|t| t["name"] == "apply_effect").unwrap()["inputSchema"]["properties"]["effect"]["enum"].clone();
        assert_eq!(effect.as_array().unwrap().len(), effects::Effect::ALL.len());
    }

    // ---- prompts and resources ----

    #[test]
    fn every_prompt_fills_in_its_argument() {
        let s = Session::default();
        let list = ask(&s, rpc(json!(1), "prompts/list", json!({})))["result"]["prompts"].clone();
        assert_eq!(list.as_array().unwrap().len(), PROMPTS.len());
        for (name, _, arg, _) in PROMPTS {
            let p = ask(&s, rpc(json!(2), "prompts/get", json!({"name": name})))["result"].clone();
            let text = p["messages"][0]["content"]["text"].as_str().unwrap().to_string();
            assert!(!text.contains('{'), "{name}: the default for {arg} is filled in: {text}");
            assert_eq!(p["messages"][0]["role"], "user");
            let p = ask(&s, rpc(json!(3), "prompts/get", json!({"name": name, "arguments": {arg: "ZZTOP"}})))["result"].clone();
            assert!(p["messages"][0]["content"]["text"].as_str().unwrap().contains("ZZTOP"), "{name}");
        }
        let bad = ask(&s, rpc(json!(4), "prompts/get", json!({"name": "nope"})));
        assert_eq!(bad["error"]["code"], -32602);
        assert_eq!(ask(&s, rpc(json!(5), "prompts/get", json!({})))["error"]["code"], -32602);
    }

    #[test]
    fn the_current_picture_is_a_resource_once_there_is_one() {
        let s = Session::default();
        let list = ask(&s, rpc(json!(1), "resources/list", json!({})))["result"]["resources"].clone();
        let uris: Vec<&str> = list.as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
        assert_eq!(uris, ["snagster://settings", "snagster://library"]);
        let none = ask(&s, rpc(json!(2), "resources/read", json!({"uri": "snagster://picture/current"})));
        assert_eq!(none["error"]["code"], -32002);
        let unknown = ask(&s, rpc(json!(3), "resources/read", json!({"uri": "snagster://nothing"})));
        assert_eq!(unknown["error"]["code"], -32002);
        assert!(unknown["error"]["message"].as_str().unwrap().contains("snagster://nothing"));

        *s.ws.lock().unwrap() = ws_with(40, 30);
        let list = ask(&s, rpc(json!(4), "resources/list", json!({})))["result"]["resources"].clone();
        assert_eq!(list[0]["uri"], "snagster://picture/current");
        assert_eq!(list[0]["mimeType"], "image/png");
        let r = ask(&s, rpc(json!(5), "resources/read", json!({"uri": "snagster://picture/current"})))["result"].clone();
        let blob = r["contents"][0]["blob"].as_str().unwrap();
        let png = base64::engine::general_purpose::STANDARD.decode(blob).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (40, 30));
    }

    // ---- tools/call ----

    #[test]
    fn tool_errors_come_back_as_error_results_not_protocol_errors() {
        let s = Session::default();
        let r = call(&s, 1, "picture_info", json!({}));
        assert_eq!(r["isError"], true);
        assert!(r["content"][0]["text"].as_str().unwrap().contains("no current picture"));
        // No name, no arguments: still an error result.
        let r = ask(&s, rpc(json!(2), "tools/call", json!({})));
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn a_json_object_answer_is_also_structured_content() {
        let dir = temp_dir("structured");
        let gif = dir.join("a.gif");
        {
            let f = std::fs::File::create(&gif).unwrap();
            let mut enc = image::codecs::gif::GifEncoder::new(f);
            for _ in 0..3 {
                let frame = image::Frame::from_parts(image::RgbaImage::from_pixel(8, 6, image::Rgba([1, 2, 3, 255])), 0, 0, image::Delay::from_numer_denom_ms(100, 1));
                enc.encode_frame(frame).unwrap();
            }
        }
        let s = Session::default();
        let r = call(&s, 1, "video_info", json!({"path": gif}));
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(r["structuredContent"]["frames"], 3);
        assert_eq!(r["structuredContent"]["width"], 8);
        assert!((r["structuredContent"]["seconds"].as_f64().unwrap() - 0.3).abs() < 0.01);
        // Plain text answers have no structured content.
        *s.ws.lock().unwrap() = ws_with(10, 10);
        let r = call(&s, 2, "picture_info", json!({}));
        assert!(r.get("structuredContent").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tools_that_need_a_picture_say_so() {
        let mut ws = Workspace::default();
        for (tool, args) in [
            ("picture_info", json!({})),
            ("view_picture", json!({})),
            ("add_mark", json!({"kind": "box", "x": 1, "y": 1, "x2": 5, "y2": 5})),
            ("list_marks", json!({})),
            ("remove_mark", json!({"index": 0})),
            ("blur_area", json!({"x": 0, "y": 0, "width": 5, "height": 5})),
            ("crop", json!({"x": 0, "y": 0, "width": 5, "height": 5})),
            ("resize", json!({"width": 5, "height": 5})),
            ("save", json!({"path": "nowhere.png"})),
            ("copy_to_clipboard", json!({})),
            ("clear_marks", json!({})),
            ("trim", json!({})),
            ("set_canvas", json!({"width": 5, "height": 5})),
            ("open_in_snagster", json!({})),
            ("read_text", json!({})),
            ("smart_redact", json!({})),
            ("update_mark", json!({"index": 0, "dx": 1})),
            ("simplify", json!({})),
            ("quick_save", json!({})),
            ("library_add", json!({})),
        ] {
            let e = err_of(ws.call(tool, &args));
            assert!(e.contains("no current picture"), "{tool}: {e}");
        }
        assert_eq!(err_of(ws.call("undo", &json!({}))), "nothing to undo");
        assert_eq!(err_of(ws.call("redo", &json!({}))), "nothing to redo");
        assert!(err_of(ws.call("no_such_tool", &json!({}))).contains("unknown tool no_such_tool"));
    }

    #[test]
    fn rectangles_need_four_numbers_and_a_size() {
        let r = rect_arg(&json!({"x": 1, "y": 2, "width": 3, "height": 4})).unwrap();
        assert_eq!((r.min.x, r.min.y, r.width(), r.height()), (1.0, 2.0, 3.0, 4.0));
        assert_eq!(rect_arg(&json!({"y": 2, "width": 3, "height": 4})).unwrap_err(), "missing number \"x\"");
        assert_eq!(rect_arg(&json!({"x": "1", "y": 2, "width": 3, "height": 4})).unwrap_err(), "missing number \"x\"");
        assert!(rect_arg(&json!({"x": 1, "y": 2, "width": 0, "height": 4})).is_err());
        assert!(rect_arg(&json!({"x": 1, "y": 2, "width": 3, "height": -4})).is_err());
        // Tools that grab the screen check their rectangle first (no screen needed to refuse).
        let mut ws = Workspace::default();
        for tool in ["capture_region", "read_screen_text", "capture_scrolling", "record_gif"] {
            assert!(err_of(ws.call(tool, &json!({"x": 0, "y": 0, "width": 0, "height": 10}))).contains("above 0"), "{tool}");
        }
        assert!(err_of(ws.call("pick_color", &json!({"y": 3}))).contains("\"x\""));
    }

    #[test]
    fn record_video_checks_its_arguments_before_recording() {
        let mut ws = Workspace::default();
        assert_eq!(err_of(ws.call("record_video", &json!({}))), "missing seconds");
        assert!(err_of(ws.call("record_video", &json!({"seconds": 1, "format": "gif"}))).contains("record_gif"));
        let e = err_of(ws.call("record_video", &json!({"seconds": 1, "format": "avi"})));
        assert!(e.contains("format is one of mp4"), "{e}");
        let area = json!({"seconds": 1, "x": 0, "y": 0, "width": 64, "height": 64, "path": "unused.mp4", "audio": "loud"});
        assert!(err_of(ws.call("record_video", &area)).contains("audio is none"));
        let area = json!({"seconds": 1, "x": 0, "y": 0, "width": -1, "height": 64});
        assert!(err_of(ws.call("record_video", &area)).contains("above 0"));
        // A GIF area is checked before recording too.
        assert!(err_of(ws.call("record_gif", &json!({"x": 0, "y": 0, "width": 10}))).contains("height"));
    }

    #[test]
    fn video_formats_are_checked_against_this_system() {
        use crate::video::VideoFormat;
        assert_eq!(video_format_arg("MP4"), Ok(VideoFormat::Mp4));
        assert!(video_format_arg("gif").unwrap_err().contains("record_gif"));
        if cfg!(windows) {
            assert_eq!(video_format_arg("hevc"), Ok(VideoFormat::Hevc));
            assert_eq!(video_format_arg("wmv"), Ok(VideoFormat::Wmv));
        } else {
            assert!(video_format_arg("hevc").unwrap_err().contains("can't be made on this system"));
        }
        // The schema offers exactly what this system makes.
        let list = tool_list();
        let rv = list["tools"].as_array().unwrap().iter().find(|t| t["name"] == "record_video").unwrap().clone();
        assert_eq!(rv["inputSchema"]["properties"]["format"]["enum"], json!(VideoFormat::video_names()));
        for k in ["audio", "microphone"] {
            assert!(rv["inputSchema"]["properties"][k].is_object(), "record_video takes {k}");
        }
        assert!(!rv["inputSchema"]["properties"]["path"]["description"].as_str().unwrap().contains("library"));
    }

    #[test]
    fn capture_options_are_read_before_anything_is_grabbed() {
        let ca = CaptureArgs::parse(&json!({})).unwrap();
        assert_eq!((ca.delay_s, ca.shape, ca.cursor, ca.library), (0.0, None, None, None));
        let ca = CaptureArgs::parse(&json!({"delay_s": 2.5, "shape": "Star", "include_cursor": true, "library": false})).unwrap();
        assert_eq!((ca.delay_s, ca.shape, ca.cursor, ca.library), (2.5, Some(CutShape::Star), Some(true), Some(false)));
        assert_eq!(CaptureArgs::parse(&json!({"shape": "rectangle"})).unwrap().shape, None, "a rectangle needs no cutting");
        assert!(CaptureArgs::parse(&json!({"delay_s": 61})).unwrap_err().contains("0-60"));
        assert!(CaptureArgs::parse(&json!({"delay_s": -1})).is_err());
        assert!(CaptureArgs::parse(&json!({"shape": "blob"})).unwrap_err().contains("shape is one of"));
        assert!(CaptureArgs::parse(&json!({"shape": "polygon", "points": [[0, 0], [5, 5]]})).unwrap_err().contains("three or more"));
        let ca = CaptureArgs::parse(&json!({"shape": "freehand", "points": [[0, 0], [9, 0], {"x": 4, "y": 8}]})).unwrap();
        assert_eq!(ca.points, vec![pos2(0.0, 0.0), pos2(9.0, 0.0), pos2(4.0, 8.0)]);
        assert!(CaptureArgs::parse(&json!({"points": [[1]]})).is_err());
        for n in SHAPE_NAMES {
            assert!(cut_shape(n).is_some(), "{n}");
        }
        // A bad option fails before the screen is touched (no screen needed for this).
        let mut ws = Workspace::default();
        for tool in ["capture_screen", "capture_window", "capture_active_window", "capture_last_area"] {
            assert!(err_of(ws.call(tool, &json!({"title": "x", "shape": "blob"}))).contains("shape"), "{tool}");
        }
        assert!(err_of(ws.call("capture_region", &json!({"x": 0, "y": 0, "width": 5, "height": 5, "delay_s": 99}))).contains("0-60"));
        assert!(err_of(ws.call("capture_last_area", &json!({}))).contains("capture_region first"));
    }

    #[test]
    fn captures_get_the_after_capture_steps_from_the_settings() {
        let dir = temp_dir("after");
        let lib = dir.join("lib");
        let grabbed = || Grabbed { image: image::RgbaImage::from_pixel(80, 60, image::Rgba([200, 0, 0, 255])), origin: (100, 200), cursor: Some((110, 230)), title: "Win".into(), app: "app.exe".into() };
        let mut ws = Workspace::with_settings(Settings { library_folder: lib.to_string_lossy().to_string(), include_cursor: true, after_library: true, ..Default::default() });
        let out = ws.finish_capture(grabbed(), "Window", &CaptureArgs::default(), &[]).unwrap();
        assert!(out[0]["text"].as_str().unwrap().contains("Kept in the library"), "{}", out[0]);
        let d = ws.doc.as_ref().unwrap();
        assert!(matches!(d.shapes[..], [Shape::Pointer { pos, .. }] if pos == pos2(10.0, 30.0)), "the pointer where the mouse was");
        let items = library::scan(&lib);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].meta.title.as_str(), items[0].meta.app.as_str(), items[0].meta.kind.as_str()), ("Win", "app.exe", "Window"));
        // The arguments win over the settings; a pointer off the picture is left out.
        let ca = CaptureArgs { cursor: Some(false), library: Some(false), shape: Some(CutShape::Ellipse), ..Default::default() };
        ws.finish_capture(grabbed(), "Area", &ca, &[]).unwrap();
        assert!(ws.doc.as_ref().unwrap().shapes.is_empty());
        assert_eq!(library::scan(&lib).len(), 1, "not added again");
        assert_eq!(ws.doc.as_ref().unwrap().image.get_pixel(0, 0).0[3], 0, "the corner is cut away");
        let mut far = grabbed();
        far.cursor = Some((5000, 5000));
        let ca = CaptureArgs { library: Some(false), ..Default::default() };
        ws.finish_capture(far, "Area", &ca, &[]).unwrap();
        assert!(ws.doc.as_ref().unwrap().shapes.is_empty());
        // Effects (a preset's) are applied: a border makes it bigger.
        ws.finish_capture(grabbed(), "Area", &ca, &[effects::Effect::Border]).unwrap();
        assert!(ws.doc.as_ref().unwrap().image.width() > 80);
        assert_eq!(ws.name, "Win");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_save_uses_the_folder_pattern_and_counter() {
        let dir = temp_dir("quick");
        let s = Settings { save_folder: dir.to_string_lossy().to_string(), name_pattern: "shot_%n".into(), format: crate::settings::ImageFormat::Jpeg, save_counter: 7, ..Default::default() };
        let mut ws = Workspace::with_settings(s);
        ws.set(Doc::new(image::RgbaImage::from_pixel(20, 10, image::Rgba([5, 5, 5, 255]))), "x").unwrap();
        let r = ws.call("quick_save", &json!({})).unwrap();
        assert!(dir.join("shot_007.jpg").exists(), "{r:?}");
        assert_eq!(r[1]["type"], "resource_link");
        ws.call("quick_save", &json!({})).unwrap();
        assert!(dir.join("shot_008.jpg").exists(), "the counter moved on and was kept");
        assert_eq!(ws.settings().save_counter, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blur_area_uses_the_block_size_and_mode_from_the_settings() {
        let check = image::RgbaImage::from_fn(40, 40, |x, y| image::Rgba([(x * 6) as u8, (y * 6) as u8, 0, 255]));
        let blurred = |block: u32, mode: crate::settings::BlurMode| {
            let mut ws = Workspace::with_settings(Settings { blur_block: block, blur_mode: mode, ..Default::default() });
            ws.set(Doc::new(check.clone()), "c").unwrap();
            ws.call("blur_area", &json!({"x": 0, "y": 0, "width": 40, "height": 40})).unwrap();
            ws.doc.unwrap().image
        };
        let big = blurred(40, crate::settings::BlurMode::Pixelate);
        assert!(big.pixels().all(|p| p == big.get_pixel(0, 0)), "one 40-pixel block");
        let small = blurred(4, crate::settings::BlurMode::Pixelate);
        assert_ne!(small.get_pixel(0, 0), small.get_pixel(4, 0), "4-pixel blocks: {:?} {:?}", small.get_pixel(0, 0), small.get_pixel(4, 0));
        let solid = blurred(12, crate::settings::BlurMode::Solid);
        assert_eq!(solid.get_pixel(20, 20).0, [28, 28, 32, 255], "the setting's mode: a solid box");
    }

    #[test]
    fn smart_redact_checks_its_kinds() {
        let mut ws = ws_with(50, 50);
        assert!(err_of(ws.call("smart_redact", &json!({"kinds": ["emails", "names"]}))).contains("unknown kind names"));
        assert!(err_of(ws.call("smart_redact", &json!({"kinds": "emails"}))).contains("list"));
        let list = tool_list();
        let t = list["tools"].as_array().unwrap().iter().find(|t| t["name"] == "smart_redact").unwrap().clone();
        assert!(t["description"].as_str().unwrap().contains("Windows only"));
        assert_eq!(t["inputSchema"]["properties"]["kinds"]["items"]["enum"], json!(ocr::RedactKinds::NAMES));
    }

    #[test]
    fn text_recognition_tools_say_they_are_windows_only() {
        let list = tool_list();
        for name in ["read_text", "read_screen_text", "smart_redact", "simplify"] {
            let t = list["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
            assert!(t["description"].as_str().unwrap().contains("Windows only"), "{name}");
        }
        for word in ["HEVC", "WMV", "record_gif", "Studio", "make_guide", "library", "Windows only"] {
            assert!(INSTRUCTIONS.contains(word), "the instructions mention {word}");
        }
    }

    #[test]
    fn every_kind_of_mark_can_be_added_and_is_described() {
        let mut ws = ws_with(400, 300);
        let two = ["arrow", "line", "box", "oval", "highlight", "spotlight"];
        let one = ["text", "callout", "step", "stamp", "pointer", "magnify"];
        for k in two.iter().chain(one.iter()) {
            let t = text_of(ws.call("add_mark", &json!({"kind": k, "x": 50, "y": 60, "x2": 150, "y2": 120, "text": "Hi"})));
            assert!(t.starts_with("Added mark"), "{k}: {t}");
        }
        assert_eq!(ws.doc.as_ref().unwrap().shapes.len(), two.len() + one.len());
        for k in one {
            assert!(ws.call("add_mark", &json!({"kind": k, "x": 5, "y": 5})).is_ok(), "{k} needs only one point");
        }
        for k in two {
            assert_eq!(err_of(ws.call("add_mark", &json!({"kind": k, "x": 5, "y": 5}))), format!("{k} needs x2 and y2"));
        }
        assert_eq!(err_of(ws.call("add_mark", &json!({"kind": "star", "x": 5, "y": 5}))), "unknown kind star");
        assert_eq!(err_of(ws.call("add_mark", &json!({"x": 5, "y": 5}))), "missing kind");
        assert_eq!(err_of(ws.call("add_mark", &json!({"kind": "box", "y": 5}))), "missing number \"x\"");
        let list = text_of(ws.call("list_marks", &json!({})));
        assert!(list.starts_with("0: arrow at "), "{list}");
        assert!(list.contains("\n1: line at ") && list.contains("\n2: box at ") && list.contains("\n3: oval at "), "{list}");
        assert!(list.contains("\n5: spotlight at 50,60 to 150,120"), "{list}");
        assert!(list.contains("text \"Hi\" at 50,60"), "{list}");
        assert!(list.contains("callout \"Hi\""), "{list}");
        assert!(list.contains("step 1 at 50,60"), "{list}");
    }

    #[test]
    fn marks_take_their_new_options() {
        let dir = temp_dir("mark-options");
        let mut ws = ws_with(400, 300);
        let mut add = |a: Value| -> Shape {
            ws.call("add_mark", &a).unwrap_or_else(|e| panic!("{a}: {e}"));
            ws.doc.as_ref().unwrap().shapes.last().unwrap().clone()
        };
        assert!(matches!(add(json!({"kind": "arrow", "x": 1, "y": 1, "x2": 50, "y2": 50, "heads": 2})), Shape::Arrow { heads: 2, .. }));
        assert!(matches!(add(json!({"kind": "arrow", "x": 1, "y": 1, "x2": 50, "y2": 50})), Shape::Arrow { heads: 1, .. }));
        assert!(matches!(add(json!({"kind": "box", "x": 1, "y": 1, "x2": 50, "y2": 50, "radius": 8})), Shape::Rect { radius: 8.0, .. }));
        match add(json!({"kind": "pen", "points": [[1, 2], [10, 20], [30, 5]], "width": 3})) {
            Shape::Pen { points, width, .. } => assert_eq!((points.len(), width), (3, 3.0)),
            other => panic!("{other:?}"),
        }
        match add(json!({"kind": "text", "x": 5, "y": 5, "text": "Hi", "font": "Arial", "bold": true, "italic": true, "align": "center", "outline": false})) {
            Shape::Text { font, bold, italic, align, outline, .. } => assert_eq!((font.as_str(), bold, italic, align, outline), ("Arial", true, true, 1, false)),
            other => panic!("{other:?}"),
        }
        match add(json!({"kind": "callout", "x": 50, "y": 50, "text": "Look", "callout_shape": "oval", "outline": true, "align": "right"})) {
            Shape::Callout { shape, outline, align, .. } => assert_eq!((shape, outline, align), (crate::settings::CalloutShape::Oval, true, 2)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(add(json!({"kind": "step", "x": 9, "y": 9, "style": "roman", "number": 4})), Shape::Step { n: 4, style: crate::settings::StepStyle::Roman, .. }));
        assert!(matches!(add(json!({"kind": "magnify", "x": 9, "y": 9, "zoom": 3})), Shape::Magnify { zoom: 3.0, .. }));
        assert!(matches!(add(json!({"kind": "magnify", "x": 9, "y": 9, "zoom": 99})), Shape::Magnify { zoom: 8.0, .. }), "zoom is kept to 1-8");
        // A picture from a file, at its own size or stretched to (x2, y2).
        let pic = dir.join("p.png");
        image::RgbaImage::from_pixel(30, 20, image::Rgba([0, 200, 0, 255])).save(&pic).unwrap();
        match add(json!({"kind": "image", "x": 100, "y": 50, "path": pic})) {
            Shape::Image { rect, image } => {
                assert_eq!((rect.min.x, rect.min.y, rect.width(), rect.height()), (100.0, 50.0, 30.0, 20.0));
                assert_eq!(image.0.dimensions(), (30, 20));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(add(json!({"kind": "image", "x": 0, "y": 0, "x2": 60, "y2": 10, "path": pic})), Shape::Image { rect, .. } if rect.width() == 60.0));
        let flat = render::compose(ws.doc.as_ref().unwrap());
        assert_eq!(flat.get_pixel(115, 60).0, [0, 200, 0, 255], "the picture is drawn");
        // And the refusals.
        for (a, want) in [
            (json!({"kind": "pen", "points": [[1, 2]]}), "two or more"),
            (json!({"kind": "pen"}), "pen needs points"),
            (json!({"kind": "pen", "points": "no"}), "pairs"),
            (json!({"kind": "arrow", "x": 1, "y": 1, "x2": 5, "y2": 5, "heads": 3}), "heads is 0, 1 or 2"),
            (json!({"kind": "text", "x": 1, "y": 1, "align": "justify"}), "align is"),
            (json!({"kind": "callout", "x": 1, "y": 1, "callout_shape": "star"}), "callout_shape is"),
            (json!({"kind": "step", "x": 1, "y": 1, "style": "greek"}), "style is"),
            (json!({"kind": "image", "x": 1, "y": 1}), "needs a path"),
            (json!({"kind": "image", "x": 1, "y": 1, "path": dir.join("none.png")}), "none.png"),
        ] {
            let e = err_of(ws.call("add_mark", &a));
            assert!(e.contains(want), "{a}: {e}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marks_are_moved_scaled_and_restyled() {
        let mut ws = ws_with(400, 300);
        ws.call("add_mark", &json!({"kind": "box", "x": 10, "y": 10, "x2": 50, "y2": 30, "width": 2})).unwrap();
        ws.call("add_mark", &json!({"kind": "text", "x": 100, "y": 100, "text": "old"})).unwrap();
        ws.call("add_mark", &json!({"kind": "step", "x": 200, "y": 200})).unwrap();
        let shape = |ws: &Workspace, i: usize| ws.doc.as_ref().unwrap().shapes[i].clone();
        let t = text_of(ws.call("update_mark", &json!({"index": 0, "dx": 5, "dy": -5, "color": "blue", "fill": true, "radius": 4})));
        assert!(t.starts_with("Mark 0 (moved, colour, fill, corners)"), "{t}");
        match shape(&ws, 0) {
            Shape::Rect { rect, color, fill, radius, .. } => {
                assert_eq!((rect.min.x, rect.min.y), (15.0, 5.0));
                assert_eq!((color, fill, radius), (Color32::from_rgb(0, 144, 255), true, 4.0));
            }
            other => panic!("{other:?}"),
        }
        // Moving to x, y puts the top-left corner of its box there.
        ws.call("update_mark", &json!({"index": 0, "x": 0, "y": 0})).unwrap();
        let b = shape(&ws, 0).bounds();
        assert_eq!((b.min.x, b.min.y), (0.0, 0.0));
        ws.call("update_mark", &json!({"index": 0, "scale": 2})).unwrap();
        assert!((shape(&ws, 0).bounds().width() - b.width() * 2.0).abs() < 3.0);
        ws.call("update_mark", &json!({"index": 1, "text": "new", "bold": true, "align": "right", "width": 4})).unwrap();
        assert!(matches!(shape(&ws, 1), Shape::Text { text, bold: true, align: 2, size, .. } if text == "new" && size == doc::text_size(4.0)));
        ws.call("update_mark", &json!({"index": 2, "number": 5, "style": "upper"})).unwrap();
        assert!(text_of(ws.call("list_marks", &json!({}))).contains("step E at"));
        // Undo puts it back as it was.
        ws.call("undo", &json!({})).unwrap();
        assert!(matches!(shape(&ws, 2), Shape::Step { n: 1, .. }));
        for (a, want) in [
            (json!({"index": 9, "dx": 1}), "no mark with that number"),
            (json!({"index": -1, "dx": 1}), "0 or more"),
            (json!({"index": 0}), "nothing to change"),
            (json!({"index": 0, "text": "x"}), "a box mark has no text"),
            (json!({"index": 1, "heads": 2}), "a text mark has no arrow heads"),
            (json!({"index": 0, "bold": true}), "a box mark has no text style"),
            (json!({"index": 0, "scale": 100}), "scale is"),
            (json!({"index": 2, "zoom": 2}), "a step mark has no zoom"),
        ] {
            let before = ws.doc.as_ref().unwrap().shapes.len();
            let e = err_of(ws.call("update_mark", &a));
            assert!(e.contains(want), "{a}: {e}");
            assert_eq!(ws.doc.as_ref().unwrap().shapes.len(), before);
        }
        // A refused change changes nothing, not even part of it.
        let before = format!("{:?}", shape(&ws, 0));
        assert!(ws.call("update_mark", &json!({"index": 0, "dx": 50, "text": "x"})).is_err());
        assert_eq!(format!("{:?}", shape(&ws, 0)), before);
    }

    #[test]
    fn steps_number_themselves_unless_told() {
        let mut ws = ws_with(200, 200);
        for _ in 0..3 {
            ws.call("add_mark", &json!({"kind": "step", "x": 10, "y": 10})).unwrap();
        }
        ws.call("add_mark", &json!({"kind": "step", "x": 10, "y": 10, "number": 9})).unwrap();
        ws.call("add_mark", &json!({"kind": "step", "x": 10, "y": 10})).unwrap();
        let ns: Vec<u32> = ws.doc.as_ref().unwrap().shapes.iter().filter_map(|s| if let Shape::Step { n, .. } = s { Some(*n) } else { None }).collect();
        assert_eq!(ns, [1, 2, 3, 9, 10]);
    }

    #[test]
    fn marks_take_their_colour_width_and_fill() {
        let mut ws = ws_with(200, 200);
        ws.call("add_mark", &json!({"kind": "box", "x": 1, "y": 2, "x2": 30, "y2": 40, "color": "#102030", "width": 3, "fill": true, "shadow": true})).unwrap();
        match &ws.doc.as_ref().unwrap().shapes[0] {
            Shape::Rect { rect, color, width, fill, shadow, .. } => {
                assert_eq!(*color, Color32::from_rgb(0x10, 0x20, 0x30));
                assert_eq!(*width, 3.0);
                assert!(*fill && *shadow);
                assert_eq!((rect.min.x, rect.min.y, rect.max.x, rect.max.y), (1.0, 2.0, 30.0, 40.0));
            }
            other => panic!("not a box: {other:?}"),
        }
        // Points given the wrong way round still make a proper box.
        ws.call("add_mark", &json!({"kind": "oval", "x": 30, "y": 40, "x2": 1, "y2": 2})).unwrap();
        let b = ws.doc.as_ref().unwrap().shapes[1].bounds();
        assert!(b.min.x <= 1.0 && b.max.x >= 30.0);
        // An empty stamp gets the tick.
        ws.call("add_mark", &json!({"kind": "stamp", "x": 50, "y": 50})).unwrap();
        assert!(matches!(&ws.doc.as_ref().unwrap().shapes[2], Shape::Stamp { emoji, .. } if emoji == "\u{2705}"));
    }

    #[test]
    fn marks_are_removed_by_number() {
        let mut ws = ws_with(100, 100);
        for x in [10, 20, 30] {
            ws.call("add_mark", &json!({"kind": "pointer", "x": x, "y": 5})).unwrap();
        }
        assert_eq!(text_of(ws.call("remove_mark", &json!({"index": 1}))), "Removed mark 1.");
        assert_eq!(ws.doc.as_ref().unwrap().shapes.len(), 2);
        assert_eq!(err_of(ws.call("remove_mark", &json!({"index": 2}))), "no mark with that number");
        assert!(err_of(ws.call("remove_mark", &json!({}))).contains("index"));
        assert_eq!(ws.doc.as_ref().unwrap().shapes.len(), 2, "failed removals change nothing");
        assert_eq!(text_of(ws.call("clear_marks", &json!({}))), "Every mark removed.");
        assert_eq!(text_of(ws.call("list_marks", &json!({}))), "No marks.");
    }

    /// Bug: `-1 as usize` is 0, so remove_mark with index -1 quietly removed the first mark.
    #[test]
    fn a_negative_mark_number_is_refused() {
        let mut ws = ws_with(100, 100);
        ws.call("add_mark", &json!({"kind": "pointer", "x": 5, "y": 5})).unwrap();
        assert!(err_of(ws.call("remove_mark", &json!({"index": -1}))).contains("0 or more"));
        assert_eq!(ws.doc.as_ref().unwrap().shapes.len(), 1);
        assert_eq!(index_arg(&json!({"i": 3}), "i"), Ok(3));
        assert_eq!(index_arg(&json!({"i": 0}), "i"), Ok(0));
        assert!(index_arg(&json!({"i": -0.5}), "i").is_err());
        assert!(index_arg(&json!({}), "i").is_err());
    }

    #[test]
    fn undo_and_redo_walk_through_changes() {
        let mut ws = ws_with(100, 80);
        ws.call("crop", &json!({"x": 0, "y": 0, "width": 50, "height": 40})).unwrap();
        ws.call("resize", &json!({"width": 20, "height": 10})).unwrap();
        let size = |ws: &Workspace| ws.doc.as_ref().unwrap().image.dimensions();
        assert_eq!(size(&ws), (20, 10));
        ws.call("undo", &json!({})).unwrap();
        assert_eq!(size(&ws), (50, 40));
        ws.call("undo", &json!({})).unwrap();
        assert_eq!(size(&ws), (100, 80));
        assert!(ws.call("undo", &json!({})).is_err());
        ws.call("redo", &json!({})).unwrap();
        assert_eq!(size(&ws), (50, 40));
        // A new change forgets what could be redone.
        ws.call("crop", &json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
        assert_eq!(err_of(ws.call("redo", &json!({}))), "nothing to redo");
    }

    #[test]
    fn undo_keeps_the_last_fifty_changes() {
        let mut ws = ws_with(50, 50);
        for i in 0..60 {
            ws.call("add_mark", &json!({"kind": "pointer", "x": i, "y": 1})).unwrap();
        }
        assert_eq!(ws.undo.len(), 50);
        for _ in 0..50 {
            ws.call("undo", &json!({})).unwrap();
        }
        assert_eq!(ws.doc.as_ref().unwrap().shapes.len(), 10);
        assert!(ws.call("undo", &json!({})).is_err());
    }

    /// Bug: opening a new picture cleared undo but not redo, so redo brought back the old picture.
    #[test]
    fn a_new_picture_cannot_redo_the_old_one() {
        let mut ws = ws_with(100, 100);
        ws.call("crop", &json!({"x": 0, "y": 0, "width": 30, "height": 30})).unwrap();
        ws.call("undo", &json!({})).unwrap();
        ws.set(Doc::new(image::RgbaImage::new(7, 9)), "other").unwrap();
        assert!(ws.call("redo", &json!({})).is_err());
        assert_eq!(ws.doc.as_ref().unwrap().image.dimensions(), (7, 9));
    }

    #[test]
    fn crop_blur_and_resize_check_their_areas() {
        let mut ws = ws_with(100, 80);
        assert_eq!(err_of(ws.call("crop", &json!({"x": 500, "y": 500, "width": 10, "height": 10}))), "that area is outside the picture");
        assert_eq!(err_of(ws.call("blur_area", &json!({"x": 500, "y": 0, "width": 10, "height": 10}))), "that area is outside the picture");
        assert_eq!(text_of(ws.call("crop", &json!({"x": 50, "y": 40, "width": 500, "height": 500}))), "Cropped to 50 x 40.");
        for mode in ["pixelate", "blur", "solid", "other"] {
            assert_eq!(text_of(ws.call("blur_area", &json!({"x": 0, "y": 0, "width": 20, "height": 20, "mode": mode}))), "Hidden.");
        }
        let px = ws.doc.as_ref().unwrap().image.get_pixel(5, 5).0;
        assert_eq!(&px[..3], &[28, 28, 32], "the last (solid) box is black");
        assert!(err_of(ws.call("resize", &json!({"width": 10}))).contains("height"));
        assert_eq!(text_of(ws.call("resize", &json!({"width": 25, "height": 20}))), "Resized to 25 x 20.");
        assert_eq!(text_of(ws.call("picture_info", &json!({}))), "test: 25 x 20 pixels, 0 marks.");
    }

    #[test]
    fn effects_are_found_by_name_or_label() {
        let mut ws = ws_with(40, 40);
        assert_eq!(err_of(ws.call("apply_effect", &json!({}))), "missing effect");
        let e = err_of(ws.call("apply_effect", &json!({"effect": "sparkles"})));
        assert!(e.starts_with("unknown effect; use one of"), "{e}");
        let first = effects::Effect::ALL[0];
        assert!(ws.call("apply_effect", &json!({"effect": format!("{first:?}").to_uppercase()})).is_ok());
        assert!(ws.call("apply_effect", &json!({"effect": first.label()})).is_ok());
    }

    #[test]
    fn view_picture_scales_to_its_limit() {
        let mut ws = ws_with(400, 200);
        let size = |o: Outcome| {
            let v = o.unwrap();
            assert_eq!(v[0]["mimeType"], "image/png");
            let png = base64::engine::general_purpose::STANDARD.decode(v[0]["data"].as_str().unwrap()).unwrap();
            let i = image::load_from_memory(&png).unwrap();
            (i.width(), i.height())
        };
        assert_eq!(size(ws.call("view_picture", &json!({}))), (400, 200), "small pictures stay as they are");
        assert_eq!(size(ws.call("view_picture", &json!({"max_size": 100}))), (100, 50));
        assert_eq!(size(ws.call("view_picture", &json!({"max_size": 1}))), (64, 32), "at least 64");
    }

    #[test]
    fn saving_needs_a_path_and_keeps_projects_editable() {
        let dir = temp_dir("save");
        let mut ws = ws_with(60, 40);
        ws.call("add_mark", &json!({"kind": "box", "x": 1, "y": 1, "x2": 20, "y2": 20})).unwrap();
        assert_eq!(err_of(ws.call("save", &json!({}))), "missing path");
        let snag = dir.join("p.snag");
        ws.call("save", &json!({"path": snag})).unwrap();
        let mut back = Workspace::default();
        back.call("open_image", &json!({"path": snag})).unwrap();
        assert_eq!(back.doc.as_ref().unwrap().shapes.len(), 1, "the mark is still a mark");
        let png = dir.join("p.png");
        ws.call("save", &json!({"path": png})).unwrap();
        assert_eq!(image::open(&png).unwrap().width(), 60);
        assert!(back.call("open_image", &json!({"path": dir.join("missing.png")})).is_err());
        assert_eq!(err_of(back.call("open_image", &json!({}))), "missing path");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colours_parse_names_hex_and_fall_back_to_red() {
        let red = Color32::from_rgb(229, 72, 77);
        assert_eq!(parse_color(None), red);
        assert_eq!(parse_color(Some("  WHITE ")), Color32::WHITE);
        assert_eq!(parse_color(Some("#abcdef")), Color32::from_rgb(0xab, 0xcd, 0xef));
        assert_eq!(parse_color(Some("#abc")), red, "short hex isn't understood");
        assert_eq!(parse_color(Some("#gggggg")), red);
        assert_eq!(parse_color(Some("chartreuse")), red);
        for n in ["red", "orange", "yellow", "green", "blue", "purple", "black", "white"] {
            assert!(n == "red" || parse_color(Some(n)) != red, "{n} is its own colour");
        }
    }

    #[test]
    fn the_command_line_knows_its_commands() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run_cli(&[]), None, "no command opens the window");
        assert_eq!(run_cli(&args(&["--some-window-flag"])), None);
        assert_eq!(run_cli(&args(&["help"])), Some(0));
        assert_eq!(run_cli(&args(&["tools"])), Some(0));
        assert_eq!(run_cli(&args(&["ocr"])), Some(2), "a picture is needed");
        assert_eq!(run_cli(&args(&["call", "no_such_tool"])), Some(1));
        let dir = temp_dir("cli");
        let src = dir.join("in.png");
        image::RgbaImage::from_pixel(12, 8, image::Rgba([9, 9, 9, 255])).save(&src).unwrap();
        let out = dir.join("out.bmp");
        assert_eq!(run_cli(&args(&["convert", src.to_str().unwrap(), "-o", out.to_str().unwrap()])), Some(0));
        assert_eq!(image::open(&out).unwrap().height(), 8);
        assert_eq!(run_cli(&args(&["convert", dir.join("missing.png").to_str().unwrap()])), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn text_of(o: Outcome) -> String {
        o.expect("the tool should have worked")[0]["text"].as_str().unwrap_or_default().to_string()
    }

    fn ws() -> Workspace {
        let mut s = Settings::default();
        s.stream.hardware = false;
        s.stream.audio = crate::audio::AudioSource::None;
        s.studio.width = 64;
        s.studio.height = 64;
        s.studio.scenes = vec![crate::studio::Scene {
            sounds: Vec::new(),
            name: "t".into(),
            items: vec![crate::studio::Item { name: "c".into(), kind: crate::studio::SourceKind::Color { color: [10, 90, 200, 255] }, x: 0.0, y: 0.0, w: 64.0, h: 64.0, visible: true }],
        }];
        s.studio.current = 0;
        Workspace::with_settings(s)
    }

    #[test]
    fn stream_services_lists_every_preset() {
        let mut w = ws();
        w.call("stream_services", &json!({})).unwrap();
        let items = w.structured.clone().unwrap()["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), crate::stream::PRESETS.len());
        assert_eq!(items[0]["service"], "twitch");
        assert!(items.iter().all(|i| i["max_fps"].as_u64().unwrap() >= 30 && i["keyframe_s"] == 2));
    }

    #[test]
    fn stream_start_checks_its_arguments_and_never_echoes_the_key() {
        let _g = crate::stream::live::test_lock();
        let mut w = ws();
        let e = w.call("stream_start", &json!({"service": "custom", "url": "ftp://example.com/live", "key": "sk_mcp_secret_1"})).unwrap_err();
        assert!(e.contains("rtmp") && !e.contains("sk_mcp_secret_1"), "{e}");
        assert!(w.call("stream_start", &json!({"service": "myspace", "key": "k"})).unwrap_err().contains("service is"));
        assert!(w.call("stream_start", &json!({"source": "window", "key": "k"})).unwrap_err().contains("title"));
        assert!(w.call("stream_start", &json!({"source": "area", "key": "k"})).is_err());
        assert_eq!(crate::stream::live::snapshot().phase, crate::stream::live::Phase::Idle, "nothing started");
    }

    #[test]
    fn stream_start_status_and_stop_against_a_local_server() {
        let _g = crate::stream::live::test_lock();
        let srv = crate::stream::testserver::TestServer::start().unwrap();
        let mut w = ws();
        let t = text_of(w.call("stream_start", &json!({"service": "custom", "url": srv.url(), "key": "sk_mcp_secret_2", "source": "studio", "fps": 15})));
        assert!(t.starts_with("Live on Custom server"), "{t}");
        assert!(!t.contains("sk_mcp_secret_2"), "{t}");
        assert_eq!(srv.publish_name(), "sk_mcp_secret_2");
        assert!(w.call("stream_start", &json!({"key": "k"})).unwrap_err().contains("already"));
        w.call("stream_status", &json!({})).unwrap();
        let st = w.structured.clone().unwrap();
        assert_eq!(st["phase"], "running");
        assert_eq!((st["width"].as_u64(), st["fps"].as_u64()), (Some(64), Some(15)));
        assert!(!st.to_string().contains("sk_mcp_secret_2"));
        let t = text_of(w.call("stream_stop", &json!({})));
        assert!(t.starts_with("The stream stopped"), "{t}");
        assert_eq!(text_of(w.call("stream_stop", &json!({}))), "Nothing is being streamed.");
        w.call("stream_status", &json!({})).unwrap();
        assert_eq!(w.structured.clone().unwrap()["phase"], "idle");
    }

    /// Multistreaming from an AI app: two local servers get the same stream, each with its own
    /// key, and neither key is ever shown back.
    #[test]
    fn stream_start_multistreams_to_every_service_in_also() {
        let _g = crate::stream::live::test_lock();
        let (a, b) = (crate::stream::testserver::TestServer::start().unwrap(), crate::stream::testserver::TestServer::start().unwrap());
        let mut w = ws();
        // Bad items are refused before anything starts.
        assert!(w.call("stream_start", &json!({"service": "custom", "url": a.url(), "key": "k1", "also": [{"service": "myspace"}]})).unwrap_err().contains("service"));
        let e = w.call("stream_start", &json!({"service": "custom", "url": a.url(), "key": "k1", "also": [{"service": "custom", "url": "ftp://x/live", "key": "sk_also_secret_0"}]})).unwrap_err();
        assert!(e.starts_with("Custom server: ") && !e.contains("sk_also_secret_0"), "{e}");
        assert_eq!(crate::stream::live::snapshot().phase, crate::stream::live::Phase::Idle);
        let t = text_of(w.call("stream_start", &json!({"service": "custom", "url": a.url(), "key": "sk_main_secret_3", "source": "studio", "fps": 15, "also": [{"service": "custom", "url": b.url(), "key": "sk_also_secret_4"}]})));
        assert!(!t.contains("sk_main_secret_3") && !t.contains("sk_also_secret_4"), "{t}");
        let start = std::time::Instant::now();
        // Both servers, not only the second: on a busy machine the first can be a little behind.
        while (a.video_messages() < 5 || b.video_messages() < 5) && start.elapsed() < std::time::Duration::from_secs(15) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!((a.publish_name(), b.publish_name()), ("sk_main_secret_3".to_string(), "sk_also_secret_4".to_string()));
        assert!(a.video_messages() >= 5 && b.video_messages() >= 5);
        w.call("stream_status", &json!({})).unwrap();
        let st = w.structured.clone().unwrap();
        assert_eq!(st["outputs"].as_array().map(Vec::len), Some(2), "{st}");
        assert_eq!(st["also"], json!(["Custom server"]));
        assert!(!st.to_string().contains("secret"), "{st}");
        assert!(text_of(w.call("stream_stop", &json!({}))).starts_with("The stream stopped"));
    }

    /// Whatever an AI app sends, the answer is a JSON-RPC reply (or nothing for a
    /// notification), never a panic. Only messages that change nothing are broken here, so a
    /// mutation can't make one that captures or saves.
    #[test]
    fn random_and_broken_messages_get_an_answer_not_a_panic() {
        let safe = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#,
            r#"{"jsonrpc":"2.0","id":"a","method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"cursor":null}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"zz_nothing","arguments":{"x":[1,2,{"y":null}]}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"zz_no_such_tool","arguments":{"path":"q","n":1e308}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99}}"#,
            r#"[{"jsonrpc":"2.0","id":5,"method":"ping"}]"#,
        ];
        let s = Session::default();
        let mut g = crate::testalloc::Bytes::new(4);
        for i in 0..3_000 {
            let input = if i % 4 == 3 {
                // Deep nesting and odd bytes as well.
                let mut v = "[".repeat(g.below(300)).into_bytes();
                v.extend(g.noise(40));
                v
            } else {
                g.mutate(safe[i % safe.len()].as_bytes())
            };
            let line = String::from_utf8_lossy(&input).into_owned();
            if let Some(reply) = s.handle(&line) {
                let v: Value = serde_json::from_str(&reply).unwrap_or_else(|e| panic!("input {i}: the reply isn't JSON ({e}): {reply}"));
                assert_eq!(v["jsonrpc"], "2.0", "{reply}");
                assert!(v.get("result").is_some() != v.get("error").is_some(), "{reply}");
            }
        }
    }
}
