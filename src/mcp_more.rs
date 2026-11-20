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
            description: "Scroll a desktop area (a web page, a document) and stitch it into one long picture, as the current picture. Uses the scrolling settings (wheel notches, longest picture, start from the top). Keep the mouse still while it runs. Where Snagster can't scroll other programs (Wayland, a Mac without the Accessibility permission, Linux without xdotool) the person scrolls instead, for up to manual_seconds.",
            schema: obj(with(with(rect(), json!({"horizontal": b("Scroll sideways instead (panorama)."), "max_steps": n("Most scroll steps, default: the setting (40)."), "manual_seconds": n("Where the person has to scroll: how long to keep taking pictures (5-600, default 30).")})), capture_props()), &["x", "y", "width", "height"]),
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
            description: "Move a picture in Snagster's library to the Recycle Bin / Trash (by the path library_search gives), with its tags. Only pictures in the library can be deleted. undo_delete brings it back, with its tags, for 2 minutes after the last delete.",
            schema: obj(json!({"path": s("The picture's path from library_search.")}), &["path"]),
            edits: false,
        },
        Tool {
            name: "undo_delete",
            description: "Bring back the library pictures library_delete moved to the Recycle Bin / Trash, with their tags, where they were. Works for 2 minutes after the last library_delete; every picture deleted in that time comes back together.",
            schema: obj(json!({}), &[]),
            edits: false,
        },
        Tool { name: "list_recordings", description: "The videos and GIFs in Snagster's recordings folder, newest first.", schema: obj(json!({"limit": n("Default 20.")}), &[]), edits: false },
        Tool { name: "video_info", description: "Read a video file back: size, frames, length (MP4, WMV) or a GIF's frames and length.", schema: obj(json!({"path": s("Full path.")}), &["path"]), edits: false },
        Tool { name: "get_settings", description: "Snagster's settings as JSON (or one of them, by key; nested ones as stream.fps). list_settings explains each one. The AI-app access token is never shown.", schema: obj(json!({"key": s("One setting's key, e.g. video_fps or stream.fps.")}), &[]), edits: false },
        Tool {
            name: "set_setting",
            description: "Change one of Snagster's settings: every tick box, choice and number on the settings pages and ribbon (list_settings gives each key, its kind, range and current value). Nested keys use a dot: stream.fps, studio.width, panel.tab. The value is checked against the setting's kind and range. Saved at once, and an open Snagster window takes it straight away. Some keys reach outside Snagster (save_folder, video_folder, update_check, start_mode...); the AI-app server's own keys (mcp_http, mcp_port, mcp_token) only the person changes.",
            schema: obj(json!({"key": s("The setting's key, e.g. video_fps, rec_format, audio_source, save_folder, stream.fps, studio.width."), "value": {"description": "The new value, as JSON (number, text, true/false, a list...)."}}), &["key", "value"]),
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
        Tool {
            name: "stream_services",
            description: "The live-streaming services Snagster knows (Twitch, YouTube, Kick, Facebook Live, Restream, a custom RTMP/RTMPS server): their usual server address, the largest picture, frame rate and bitrate they take, and where to find the stream key.",
            schema: obj(json!({}), &[]),
            edits: false,
        },
        Tool {
            name: "stream_start",
            description: "Go live: stream the screen, a window, a desktop area or the Studio scene to a service over RTMP/RTMPS, in the background (it keeps going after this call; stop it with stream_stop). Uses the stream key saved in Snagster's settings unless key is given; the key is never shown back. Returns once the stream is on the air (or after 15 s still connecting). One stream at a time. Defaults come from the Streaming settings.",
            schema: obj(
                json!({
                    "service": {"type": "string", "enum": ["twitch", "youtube", "kick", "facebook", "restream", "custom"], "description": "Default: the setting."},
                    "url": s("Server address (rtmp:// or rtmps://); default: the setting, or the service's usual server."),
                    "key": s("The stream key, only if it should not come from the settings. Secret: it is never repeated in answers."),
                    "source": {"type": "string", "enum": ["screen", "window", "area", "studio"], "description": "What to stream; default: the setting."},
                    "title": s("Part of the window title (source window)."),
                    "x": n("Desktop x (source area)."), "y": n("Desktop y."), "width": n("Desktop width."), "height": n("Desktop height."),
                    "fps": n("Frames per second (the service's limit applies)."),
                    "bitrate": n("Video bitrate in kbit/s (0: recommended; the service's limit applies)."),
                    "max_height": n("The tallest picture sent, e.g. 720 (0: the source's size)."),
                    "audio": {"type": "string", "enum": ["none", "system", "microphone", "both"], "description": "Sound to stream (Windows); default: the setting."},
                    "record_copy": b("Also save what is streamed as an FLV file in the recordings folder."),
                    "hevc": b("Send HEVC (H.265) where the service takes it (YouTube and custom servers, over Enhanced RTMP); others get H.264. Default: the setting."),
                    "hardware": b("Encode on the graphics chip (Intel, AMD or NVIDIA). Default: the setting."),
                    "keyframe_s": n("Seconds between keyframes, 1-4 (services usually want 2). Default: the setting."),
                    "reconnects": n("Reconnect this many times when the connection drops, 0-50. Default: the setting."),
                    "cursor": b("Show the mouse pointer in the stream. Default: the setting."),
                    "also": {
                        "type": "array",
                        "maxItems": 4,
                        "description": "Multistream: more services to send the same stream to at once (encoded once; the picture is fitted to what every service takes). Default: the extra services switched on in Settings > Streaming, with their saved keys. [] streams to the main service only.",
                        "items": {"type": "object", "properties": {
                            "service": {"type": "string", "enum": ["twitch", "youtube", "kick", "facebook", "restream", "custom"]},
                            "url": s("Server address; default: the service's usual one (or the one saved for this service)."),
                            "key": s("Stream key; default: the one saved for this service in the settings. Secret: never repeated.")
                        }, "required": ["service"]}
                    }
                }),
                &[],
            ),
            edits: false,
        },
        Tool { name: "stream_status", description: "Whether Snagster is live, and how it goes: state, time on the air, bitrate, frames sent and dropped, reconnects, encoder, and the last error (the key masked); when multistreaming, outputs lists each service with its own state, bitrate, frames and error.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "stream_stats", description: "How well the live stream is going, measured over the last 10 seconds: per service the bitrate really sent against the bitrate set, frames the network dropped (count since going live and share), how far its upload queue is behind (ms), how long the connection has been up and reconnects (in all and in the last 10 minutes); for the computer the frames it dropped (encoder or late) and the encoder's time per frame against the frame's budget; and warnings in plain words (upload too slow, computer too slow, connection dropping) with their level; level is the worst of them (ok, warning or bad). measuring is true for the first seconds, before the rates are known.", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "stream_stop", description: "End the live stream (the last packets are sent and the stream is closed); returns its final numbers and the copy's file when one was recorded.", schema: obj(json!({}), &[]), edits: false },
        Tool {
            name: "replay_start",
            description: "Turn the replay buffer on: Snagster keeps encoding the screen the mouse is on (on the graphics chip when it can) and keeps the last 30-120 seconds in memory, so replay_save can save a moment after it happened. It runs in the background until replay_stop. Off by default. On Wayland the desktop may ask the person to choose a screen. Frame rate, quality, encoder and pointer come from the Recording settings; sound from replay_audio.",
            schema: obj(json!({"seconds": {"type": "integer", "minimum": 30, "maximum": 120, "description": "How much to keep, 30-120 s (default: the replay_s setting)."}}), &[]),
            edits: false,
        },
        Tool {
            name: "replay_save",
            description: "Save the last seconds the replay buffer kept as an MP4 in the recordings folder, at once (the buffer keeps going). The clip starts at a keyframe, so it can be up to 2 s longer than asked. When this server's replay buffer is off but the Snagster window has one on, the window saves its own.",
            schema: obj(json!({"seconds": {"type": "integer", "minimum": 2, "maximum": 120, "description": "Save only the last this many seconds (default: everything kept)."}}), &[]),
            edits: false,
        },
        Tool { name: "replay_stop", description: "Turn the replay buffer off; what it kept is thrown away (nothing is saved).", schema: obj(json!({}), &[]), edits: false },
        Tool { name: "replay_status", description: "Whether the replay buffer is on: its phase (off, starting, on), the seconds it keeps and holds now, the picture size, frame rate, encoder, whether sound is kept, and the last error.", schema: obj(json!({}), &[]), edits: false },
    ]
}

/// replay_status's answer.
pub(super) fn replay_json(s: &crate::replay::Status) -> Value {
    let phase = match s.phase {
        crate::replay::Phase::Off => "off",
        crate::replay::Phase::Starting => "starting",
        crate::replay::Phase::On => "on",
    };
    let mut v = json!({"phase": phase, "on": s.on(), "seconds": s.seconds, "held_s": (s.held_s * 10.0).round() / 10.0});
    if let Some(i) = &s.info {
        v["width"] = json!(i.width);
        v["height"] = json!(i.height);
        v["fps"] = json!(i.fps);
        v["encoder"] = json!(i.encoder);
        v["hardware"] = json!(i.hardware);
        v["sound"] = json!(i.sound);
    }
    if let Some(e) = &s.error {
        v["error"] = json!(e);
    }
    v
}

/// An integer argument within `lo..=hi`, or None when it isn't given.
fn int_in(a: &Value, k: &str, lo: u32, hi: u32) -> Result<Option<u32>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if n.fract() == 0.0 && n >= lo as f64 && n <= hi as f64 => Ok(Some(n as u32)),
            _ => Err(format!("{k} is a whole number from {lo} to {hi}")),
        },
    }
}

impl Workspace {
    fn replay_start(&mut self, a: &Value) -> Outcome {
        use crate::replay::{self, Phase};
        let seconds = int_in(a, "seconds", 30, 120)?;
        let s = replay::status();
        if s.on() {
            if let Some(n) = seconds {
                replay::set_seconds(n);
            }
            return Ok(vec![text(format!("Replay on: keeping the last {} s (already on). Save it with replay_save.", seconds.unwrap_or(s.seconds)))]);
        }
        let mut o = replay::ReplayOptions::from_settings(&self.settings());
        if let Some(n) = seconds {
            o.seconds = n;
        }
        replay::begin(o)?;
        let s = self.ticking(Some(120.0), "Turning replay on", || replay::wait_started(std::time::Duration::from_secs(130)));
        match s.phase {
            Phase::On => {
                let i = s.info.clone();
                let what = i.map(|i| format!(" ({} x {} at {} fps, {}{})", i.width, i.height, i.fps, i.encoder, if i.sound { ", with sound" } else { "" })).unwrap_or_default();
                Ok(vec![text(format!("Replay on: keeping the last {} s of the screen{what}. Save it with replay_save.", s.seconds))])
            }
            Phase::Starting => Ok(vec![text("Replay is still starting (the desktop may be asking which screen to share); check with replay_status.")]),
            Phase::Off => Err(s.error.unwrap_or_else(|| "Replay couldn't start.".into())),
        }
    }

    fn replay_save(&mut self, a: &Value) -> Outcome {
        use crate::replay;
        let seconds = int_in(a, "seconds", 2, 120)?;
        if !replay::status().on() {
            // The window's own buffer (this server runs apart from it over stdio).
            #[cfg(not(test))]
            if seconds.is_none() && crate::instance::send_command("replay_save", None).is_ok() {
                return Ok(vec![text("Asked the Snagster window to save its replay: when it is on there, the video goes to the recordings folder (list_recordings shows it).")]);
            }
            return Err("Replay is off; call replay_start first.".into());
        }
        let v = replay::save(seconds)?;
        let msg = format!("Saved {}: the last {:.1} s ({} frames{}).", v.path.display(), v.seconds, v.frames, if v.audio { ", with sound" } else { "" });
        Ok(saved(&v.path, msg))
    }
}

impl Workspace {
    /// stream_start: the Streaming settings with the arguments on top; the key from the key
    /// store unless given. Every message has the key masked.
    fn stream_start(&mut self, a: &Value) -> Outcome {
        use crate::stream::live::{self, LiveOptions, Phase, StudioFrames};
        use crate::stream::{Source, mask};
        let mut s = self.settings();
        stream_args(&mut s, a)?;
        let key = match str_arg(a, "key") {
            Some(k) => k.trim().to_string(),
            None => crate::stream::keystore::load().unwrap_or_default(),
        };
        let saved_key = |id: u32| crate::stream::keystore::load_for(id).unwrap_or_default();
        let mut o = LiveOptions::from_settings(&s, &key, saved_key);
        if let Some(list) = a.get("also") {
            o.also = also_arg(list, &s.stream.also, &saved_key)?;
        }
        let keys: Vec<String> = std::iter::once(&o.target).chain(&o.also).map(|t| t.key.clone()).collect();
        let mask_keys = |e: &str| keys.iter().fold(e.to_string(), |e, k| mask(&e, k));
        // Check the addresses and the keys before anything starts (no network yet).
        o.target.endpoint().map_err(|e| {
            let e = mask(&e, &key);
            if key.is_empty() { format!("{e} (no stream key is saved: add it in Snagster's Settings > Streaming, or pass key)") } else { e }
        })?;
        for t in &o.also {
            t.endpoint().map_err(|e| {
                let e = format!("{}: {}", t.service.preset().name, mask(&e, &t.key));
                if t.key.is_empty() { format!("{e} (no key is saved for it: add it under Also stream to in Settings > Streaming, or pass also[].key)") } else { e }
            })?;
        }
        let studio = (o.source == Source::Studio).then(|| StudioFrames::Start(s.studio.clone()));
        live::begin(o, studio, crate::stream::Encoders::system_picked(&s.video_encoder)).map_err(|e| mask_keys(&e))?;
        let stop = self.stop.clone();
        let cancelled = self.ticking(Some(15.0), "Going live", || {
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_secs(15) {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return true;
                }
                let snap = live::snapshot();
                if snap.live() || snap.phase == Phase::Idle || snap.stats.as_ref().is_some_and(|s| s.state == crate::stream::State::Failed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            false
        });
        if cancelled {
            live::end();
            return Err("cancelled".into());
        }
        let snap = live::snapshot();
        let v = stream_json(&snap);
        if snap.phase == Phase::Idle {
            return Err(format!("Couldn't go live: {}", snap.error.as_deref().map(mask_keys).unwrap_or_else(|| "it stopped at once".into())));
        }
        if let Some(e) = snap.stats.as_ref().filter(|s| s.state == crate::stream::State::Failed).and_then(|s| s.error.clone()) {
            live::end();
            return Err(format!("Couldn't go live: {}", mask_keys(&e)));
        }
        let mut line = match &snap.info {
            Some(i) if snap.live() && i.also.is_empty() => format!("Live on {} ({}): {}, {} x {} at {} fps, {} kbit/s. Stop it with stream_stop.", i.service, i.address, i.source, i.width, i.height, i.fps, i.kbps),
            Some(i) if snap.live() => format!("Live on {}: {}, {} x {} at {} fps, {} kbit/s. Stop it with stream_stop.", i.services(), i.source, i.width, i.height, i.fps, i.kbps),
            Some(i) => format!("Still connecting to {} ({}); check with stream_status.", i.services(), i.address),
            None => "Still starting; check with stream_status.".to_string(),
        };
        // A service that isn't live (yet) when multistreaming.
        if let Some(st) = snap.stats.as_ref().filter(|s| s.outputs.len() > 1) {
            for out in st.outputs.iter().filter(|o| o.state != crate::stream::State::Live) {
                line.push_str(&format!(" {}: {:?}{}.", out.service, out.state, out.error.as_ref().map(|e| format!(" ({e})")).unwrap_or_default()));
            }
        }
        self.structured = Some(v);
        Ok(vec![text(mask_keys(&line))])
    }
}

/// stream_start's `also`: the services to multistream to, each key given or the one saved for
/// that service (the first extra service of the settings with the same service).
/// stream_start's arguments on top of the Streaming settings (for this stream only; nothing is
/// saved). Every option on the Streaming page has an argument here.
fn stream_args(s: &mut crate::settings::Settings, a: &Value) -> Result<(), String> {
    use crate::stream::{Service, Source};
    if let Some(v) = str_arg(a, "service") {
        s.stream.service = Service::parse(v).ok_or("service is twitch, youtube, kick, facebook, restream or custom")?;
        s.stream.url.clear();
    }
    if let Some(u) = str_arg(a, "url") {
        s.stream.url = u.trim().to_string();
    }
    if let Some(v) = str_arg(a, "source") {
        s.stream.source = match Source::parse(v).ok_or("source is screen, window, area or studio")? {
            Source::Window(_) => Source::Window(str_arg(a, "title").ok_or("source window needs the title (part of it is enough)")?.to_string()),
            Source::Area(..) => {
                let r = rect_arg(a)?;
                Source::Area(r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32)
            }
            other => other,
        };
    }
    if let Some(f) = num(a, "fps") {
        s.stream.fps = f.clamp(1.0, 120.0) as u32;
    }
    if let Some(k) = num(a, "bitrate") {
        s.stream.kbps = k.clamp(0.0, 100_000.0) as u32;
    }
    if let Some(h) = num(a, "max_height") {
        s.stream.max_height = h.clamp(0.0, 4320.0) as u32;
    }
    if let Some(x) = str_arg(a, "audio") {
        s.stream.audio = crate::audio::AudioSource::parse(x).ok_or("audio is none, system, microphone or both")?;
    }
    if let Some(c) = bool_arg(a, "record_copy") {
        s.stream.record_copy = c;
    }
    if let Some(v) = bool_arg(a, "hevc") {
        s.stream.hevc = v;
    }
    if let Some(v) = bool_arg(a, "hardware") {
        s.stream.hardware = v;
    }
    if let Some(v) = bool_arg(a, "cursor") {
        s.stream.cursor = v;
    }
    if let Some(k) = num(a, "keyframe_s") {
        if !(1.0..=4.0).contains(&k) {
            return Err("keyframe_s is 1-4 seconds".into());
        }
        s.stream.keyframe_s = k.round() as u32;
    }
    if let Some(r) = num(a, "reconnects") {
        if !(0.0..=50.0).contains(&r) {
            return Err("reconnects is 0-50".into());
        }
        s.stream.reconnects = r.round() as u32;
    }
    Ok(())
}

fn also_arg(list: &Value, saved: &[crate::settings::ExtraService], saved_key: &dyn Fn(u32) -> String) -> Result<Vec<crate::stream::StreamTarget>, String> {
    use crate::stream::{Service, StreamTarget};
    let items = list.as_array().ok_or("also is a list of {service, url, key}")?;
    if items.len() > crate::settings::MAX_ALSO {
        return Err(format!("also takes at most {} services", crate::settings::MAX_ALSO));
    }
    items
        .iter()
        .map(|it| {
            let service = it.get("service").and_then(Value::as_str).and_then(Service::parse).ok_or("each also item needs service: twitch, youtube, kick, facebook, restream or custom")?;
            let known = saved.iter().find(|x| x.service == service);
            let url = it.get("url").and_then(Value::as_str).map(|u| u.trim().to_string()).or_else(|| known.map(|k| k.url.clone())).unwrap_or_default();
            let key = it.get("key").and_then(Value::as_str).map(|k| k.trim().to_string()).or_else(|| known.map(|k| saved_key(k.id))).unwrap_or_default();
            Ok(StreamTarget { service, url, key })
        })
        .collect()
}

/// The stream's state as JSON (no secret in it).
fn stream_json(s: &crate::stream::live::Snapshot) -> Value {
    use crate::stream::live::Phase;
    let phase = match s.phase {
        Phase::Idle => "idle",
        Phase::Starting => "starting",
        Phase::Running => "running",
        Phase::Stopping => "stopping",
    };
    let mut v = json!({"phase": phase, "live": s.live(), "error": s.error});
    if let Some(i) = &s.info {
        v["service"] = json!(i.service);
        v["address"] = json!(i.address);
        v["source"] = json!(i.source);
        v["width"] = json!(i.width);
        v["height"] = json!(i.height);
        v["fps"] = json!(i.fps);
        v["video_kbps"] = json!(i.kbps);
        v["also"] = json!(i.also);
        v["copy"] = json!(i.copy.as_ref().map(|p| p.display().to_string()));
    }
    if let Some(st) = &s.stats {
        v["state"] = json!(format!("{:?}", st.state).to_lowercase());
        v["uptime_s"] = json!((st.uptime_s * 10.0).round() / 10.0);
        v["bitrate_bps"] = json!(st.bitrate_bps);
        v["frames_sent"] = json!(st.frames_sent);
        v["frames_dropped"] = json!(st.frames_dropped);
        v["reconnects"] = json!(st.reconnects);
        v["queue_ms"] = json!(st.queue_ms);
        v["encoder"] = json!(st.video_encoder.as_ref().map(|e| format!("{} ({})", e.name, if e.hardware { "graphics chip" } else { "processor" })));
        v["notice"] = json!(st.notice);
        v["outputs"] = json!(st.outputs.iter().map(|o| json!({
            "service": o.service,
            "address": o.address,
            "state": format!("{:?}", o.state).to_lowercase(),
            "bitrate_bps": o.bitrate_bps,
            "frames_sent": o.frames_sent,
            "frames_dropped": o.frames_dropped,
            "reconnects": o.reconnects,
            "error": o.error,
        })).collect::<Vec<_>>());
        if st.error.is_some() {
            v["error"] = json!(st.error);
        }
    }
    v
}

/// stream_stats: the stream's health (`stream::health`), with plain-words warnings.
fn stats_json(s: &crate::stream::live::Snapshot) -> Value {
    use crate::stream::health::Level;
    let round = |x: f32, places: i32| {
        let f = 10f64.powi(places);
        (x as f64 * f).round() / f
    };
    let mut v = json!({"live": s.live()});
    let Some(st) = s.stats.as_ref() else {
        v["message"] = json!("Nothing is being streamed.");
        return v;
    };
    let h = &st.health;
    v["state"] = json!(format!("{:?}", st.state).to_lowercase());
    v["level"] = json!(match h.level() {
        None => "ok",
        Some(Level::Warning) => "warning",
        Some(Level::Bad) => "bad",
    });
    v["measuring"] = json!(h.window_s == 0.0);
    v["window_s"] = json!(round(h.window_s, 1));
    v["uptime_s"] = json!((st.uptime_s * 10.0).round() / 10.0);
    v["set_kbps"] = json!(h.set_kbps);
    v["encoder"] = json!(st.video_encoder.as_ref().map(|e| format!("{} ({})", e.name, if e.hardware { "graphics chip" } else { "processor" })));
    v["computer"] = json!({
        "dropped_frames": h.encoder_lost,
        "dropped_share": round(h.encoder_lost_share, 4),
        "encode_ms": round(h.encode_ms, 2),
        "frame_budget_ms": round(h.frame_budget_ms, 2),
        "encoder_load": round(h.encoder_load, 3),
    });
    v["outputs"] = json!(h.outputs.iter().zip(&st.outputs).map(|(o, out)| json!({
        "service": o.service,
        "state": format!("{:?}", out.state).to_lowercase(),
        "measuring": !o.measured,
        "sent_kbps": o.sent_kbps,
        "network_dropped_frames": o.dropped,
        "network_dropped_share": round(o.dropped_share, 4),
        "queue_ms": o.queue_ms,
        "connected_s": (o.connected_s * 10.0).round() / 10.0,
        "reconnects": o.reconnects,
        "reconnects_last_10_min": o.reconnects_recent,
    })).collect::<Vec<_>>());
    v["warnings"] = json!(h.warnings.iter().map(|w| json!({
        "problem": format!("{:?}", w.problem).to_lowercase(),
        "level": if w.level == Level::Bad { "bad" } else { "warning" },
        "service": w.service,
        "message": w.fill(w.advice()),
    })).collect::<Vec<_>>());
    v
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

/// A scrolling capture for AI apps (capture_scrolling and scrolling presets): the window's
/// scrolling settings, at most 200 steps. Where Snagster can't scroll other programs the person
/// scrolls, and the pictures stop after `manual_seconds` (default 30) since nobody presses Stop.
fn scroll_opts(s: &crate::settings::Settings, horizontal: bool, max_steps: Option<f64>, manual_seconds: Option<f64>, can_scroll: bool) -> capture::ScrollOptions {
    let mut o = capture::ScrollOptions::from_settings(s, horizontal, false, can_scroll, capture::detect::Space::default());
    o.max_steps = max_steps.unwrap_or(s.scroll_max_steps as f64).clamp(1.0, 200.0) as u32;
    if o.manual {
        o.time_limit = Some(std::time::Duration::from_secs(manual_seconds.unwrap_or(30.0).clamp(5.0, 600.0) as u64));
    }
    o
}

/// What the progress notes say while scrolling.
fn scroll_what(o: &capture::ScrollOptions) -> &'static str {
    if o.manual { "Snagster can't scroll other programs here: scroll the area yourself now" } else { "Scrolling and stitching" }
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
                let o = scroll_opts(&s, horizontal, num(a, "max_steps"), num(a, "manual_seconds"), capture::can_scroll_itself());
                let area = (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32);
                self.wait(ca.delay_s)?;
                let img = self.ticking(None, scroll_what(&o), || capture::scroll_capture(area, &o, self.stop.clone(), None))?;
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
                    imgs.push(export::open_picture(Path::new(p))?);
                }
                let vertical = a.get("vertical").and_then(Value::as_bool).unwrap_or(false);
                let out = effects::combine(&imgs, vertical, 16).ok_or("the combined picture would be too big (over 268 million pixels)")?;
                self.set(Doc::new(out), "Combined")
            }
            "make_guide" => {
                let title = str_arg(a, "title").unwrap_or("Guide").to_string();
                let path = PathBuf::from(str_arg(a, "path").ok_or("missing path")?);
                let list = a.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
                let current = a.get("include_current").and_then(Value::as_bool).unwrap_or(false);
                let total = (list.len() + current as usize + 1) as f64;
                let mut steps: Vec<(String, image::RgbaImage)> = Vec::new();
                // A one-picture guide shrinks each step to the page as it is read (as the page
                // would), and no guide holds more pictures than fit in memory at once.
                let one_picture = path.extension().is_some_and(|e| ["png", "jpg", "jpeg"].iter().any(|x| e.eq_ignore_ascii_case(x)));
                let mut held = 0u64;
                for (i, s) in list.iter().enumerate() {
                    let p = s.get("path").and_then(Value::as_str).ok_or("each step needs a path")?;
                    let mut img = export::open_picture(Path::new(p))?;
                    if one_picture {
                        img = effects::fit_guide_width(img, export::GUIDE_WIDTH);
                    }
                    held += img.width() as u64 * img.height() as u64;
                    if held > 4 * effects::GUIDE_PAGE_MAX {
                        return Err(format!("the pictures are too big together for one guide (step {} goes over); use fewer or smaller steps", i + 1));
                    }
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
                    let now = std::time::Instant::now();
                    let deleted = self.deleted.get_or_insert_with(|| crate::recycle::Pending::new(crate::recycle::MCP_UNDO_FOR, std::sync::Arc::new(crate::recycle::SystemBin)));
                    let bin = deleted.bin();
                    let g = crate::recycle::remove(&item.path, Some(item.meta.clone()), crate::recycle::way_here(), bin.as_ref())?;
                    for old in deleted.add(g, now) {
                        let _ = crate::recycle::finish(&old, bin.as_ref());
                    }
                    let n = deleted.count();
                    let where_to = crate::platform::trash_text("the Recycle Bin", "the Trash");
                    let also = if n > 1 { format!(" ({n} pictures deleted in a row come back together)") } else { String::new() };
                    self.structured = Some(json!({"path": item.path, "undo": "undo_delete", "undo_for_seconds": crate::recycle::MCP_UNDO_FOR.as_secs(), "waiting": n}));
                    return Ok(vec![text(format!(
                        "Moved {} to {where_to}. To undo, call undo_delete within {} seconds: it comes back where it was, with its tags{also}.",
                        item.path.display(),
                        crate::recycle::MCP_UNDO_FOR.as_secs()
                    ))]);
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
            "undo_delete" => {
                let now = std::time::Instant::now();
                let Some(deleted) = self.deleted.as_mut().filter(|d| d.showing(now)) else {
                    return Err(format!(
                        "nothing to undo: no library_delete in the last {} seconds (the Recycle Bin / Trash still has what was deleted earlier)",
                        crate::recycle::MCP_UNDO_FOR.as_secs()
                    ));
                };
                let (back, failed) = deleted.undo_now(now);
                let errors: Vec<String> = failed.iter().map(|(n, e)| format!("{n}: {e}")).collect();
                self.structured = Some(json!({"restored": back, "errors": errors}));
                let names = back.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
                if back.is_empty() {
                    return Err(format!("couldn't restore it: {}", errors.join("; ")));
                }
                let mut out = vec![text(if errors.is_empty() {
                    format!("Restored {names}, with its tags.")
                } else {
                    format!("Restored {names}, with its tags. Couldn't restore: {}", errors.join("; "))
                })];
                out.extend(back.iter().map(|p| link(p)));
                Ok(out)
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
                let v = super::masked_settings(&self.settings())?;
                match str_arg(a, "key") {
                    Some(k) => {
                        let found = k.split('.').try_fold(&v, |v, part| v.get(part)).ok_or(format!("there's no setting {k}"))?;
                        Ok(vec![json_text(found)])
                    }
                    None => Ok(vec![json_text(&v)]),
                }
            }
            "set_setting" => {
                let key = str_arg(a, "key").ok_or("missing key")?;
                let value = a.get("value").cloned().ok_or("missing value")?;
                let spec = crate::catalog::check_setting(key, &value)?;
                let mut v = serde_json::to_value(self.settings()).map_err(|e| e.to_string())?;
                let mut slot = &mut v;
                for part in key.split('.') {
                    slot = slot.get_mut(part).ok_or(format!("there's no setting {key} (see list_settings)"))?;
                }
                *slot = value;
                let s: crate::settings::Settings = serde_json::from_value(v).map_err(|e| format!("that value doesn't fit {key}: {e}"))?;
                self.save_settings(&s);
                let note = if spec.who == crate::catalog::Who::Outside { " (this reaches outside Snagster)" } else { "" };
                Ok(vec![text(format!("Set {key}{note}."))])
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
                    ) + &if d.note.is_empty() { String::new() } else { format!(" {}.", d.note) },
                ))
            }
            "stream_services" => {
                let items: Vec<Value> = crate::stream::PRESETS
                    .iter()
                    .map(|p| {
                        json!({
                            "service": format!("{:?}", p.service).to_lowercase(), "name": p.name, "url": p.url, "key_help": p.key_help,
                            "max_width": p.max_width, "max_height": p.max_height, "max_fps": p.max_fps, "max_video_kbps": p.max_video_kbps,
                            "max_pixel_rate": p.max_pixel_rate, "keyframe_s": p.keyframe_s, "audio_kbps": p.audio_kbps, "hevc": p.hevc
                        })
                    })
                    .collect();
                self.structured = Some(json!({ "items": items }));
                Ok(vec![json_text(&items)])
            }
            "stream_status" => {
                let v = stream_json(&crate::stream::live::snapshot());
                self.structured = Some(v.clone());
                Ok(vec![json_text(&v)])
            }
            "stream_stats" => {
                let v = stats_json(&crate::stream::live::snapshot());
                self.structured = Some(v.clone());
                Ok(vec![json_text(&v)])
            }
            "stream_stop" => {
                use crate::stream::live;
                if !live::end() {
                    return Ok(vec![text("Nothing is being streamed.")]);
                }
                self.ticking(Some(8.0), "Stopping the stream", || live::wait_idle(std::time::Duration::from_secs(8)));
                let s = live::snapshot();
                let mut line = "The stream stopped.".to_string();
                if let Some(e) = &s.last {
                    line = format!(
                        "The stream stopped after {:.0} s: {} frames sent, {} dropped, {} reconnects.",
                        e.stats.uptime_s, e.stats.frames_sent, e.stats.frames_dropped, e.stats.reconnects
                    );
                    match &e.copy {
                        Some(Ok(c)) => line += &format!(" Copy: {} ({} frames).", c.path.display(), c.video_frames),
                        Some(Err(err)) => line += &format!(" The copy failed: {err}"),
                        None => {}
                    }
                }
                Ok(vec![text(line)])
            }
            "stream_start" => self.stream_start(a),
            "replay_start" => self.replay_start(a),
            "replay_save" => self.replay_save(a),
            "replay_stop" => Ok(vec![text(if crate::replay::end() { "Replay off." } else { "Replay was already off." })]),
            "replay_status" => {
                let v = replay_json(&crate::replay::status());
                self.structured = Some(v.clone());
                Ok(vec![json_text(&v)])
            }
            other => self.call_app(other, a),
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
                let o = scroll_opts(&s, false, None, None, capture::can_scroll_itself());
                let img = self.ticking(None, scroll_what(&o), || capture::scroll_capture(r, &o, self.stop.clone(), None))?;
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

    #[test]
    fn stream_start_takes_every_streaming_option() {
        let mut s = crate::settings::Settings::default();
        s.stream.hevc = false;
        s.stream.hardware = true;
        s.stream.cursor = true;
        let a = json!({"service": "youtube", "source": "screen", "fps": 60, "bitrate": 9000, "max_height": 1080, "audio": "both", "record_copy": true,
            "hevc": true, "hardware": false, "cursor": false, "keyframe_s": 4, "reconnects": 10});
        stream_args(&mut s, &a).unwrap();
        let st = &s.stream;
        assert_eq!(st.service, crate::stream::Service::YouTube);
        assert_eq!(st.source, crate::stream::Source::Screen);
        assert_eq!((st.fps, st.kbps, st.max_height, st.keyframe_s, st.reconnects), (60, 9000, 1080, 4, 10));
        assert!(st.hevc && !st.hardware && !st.cursor && st.record_copy);
        assert_eq!(st.audio, crate::audio::AudioSource::Both);
        // Left out: the setting stays.
        let before = s.stream.clone();
        stream_args(&mut s, &json!({})).unwrap();
        assert_eq!(s.stream, before);
        assert!(stream_args(&mut s, &json!({"keyframe_s": 9})).unwrap_err().contains("1-4"));
        assert!(stream_args(&mut s, &json!({"reconnects": 99})).unwrap_err().contains("0-50"));
        assert!(stream_args(&mut s, &json!({"source": "window"})).unwrap_err().contains("title"));
        // Every Streaming setting an AI app may change can be given to stream_start directly
        // (also: the list; url and service: the server; source: what is streamed).
        let props = tools().into_iter().find(|t| t.name == "stream_start").unwrap().schema["properties"].clone();
        for k in crate::catalog::SETTINGS.iter().filter_map(|k| k.key.strip_prefix("stream.")) {
            let arg = match k {
                "kbps" => "bitrate",
                other => other,
            };
            assert!(props.get(arg).is_some(), "stream_start has no argument for stream.{k}");
        }
    }

    #[test]
    fn ai_scrolling_uses_the_windows_scroll_settings() {
        // Before, capture_scrolling ignored these three settings and never fell back to manual.
        let s = crate::settings::Settings { scroll_notches: 9, scroll_max_height: 7000, scroll_from_top: false, scroll_wait_ms: 222, scroll_max_steps: 33, ..Default::default() };
        let o = scroll_opts(&s, true, None, None, true);
        assert!(o.horizontal && !o.manual && !o.from_start);
        assert_eq!((o.notches, o.max_length, o.delay_ms, o.max_steps, o.time_limit), (9, 7000, 222, 33, None));
        assert_eq!(scroll_what(&o), "Scrolling and stitching");
        assert_eq!(scroll_opts(&s, false, Some(500.0), None, true).max_steps, 200, "at most 200 steps");
        assert_eq!(scroll_opts(&s, false, Some(0.0), None, true).max_steps, 1);
        // Wayland and the like: the person scrolls, and it stops by itself.
        let m = scroll_opts(&s, false, None, None, false);
        assert!(m.manual);
        assert_eq!(m.time_limit, Some(std::time::Duration::from_secs(30)));
        assert!(scroll_what(&m).contains("scroll the area yourself"));
        assert_eq!(scroll_opts(&s, false, None, Some(1.0), false).time_limit, Some(std::time::Duration::from_secs(5)));
        assert_eq!(scroll_opts(&s, false, None, Some(9999.0), false).time_limit, Some(std::time::Duration::from_secs(600)));
    }

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
        st.scenes.push(crate::studio::Scene { sounds: Vec::new(), name: "Two".into(), items: Vec::new() });
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
        assert!(ws.call("set_setting", &json!({"key": "video_fps", "value": "lots"})).unwrap_err().contains("video_fps takes a whole number 1-120"));
        assert!(ws.call("set_setting", &json!({"key": "bindings", "value": 5})).unwrap_err().contains("doesn't fit bindings"));
        assert!(ws.call("set_setting", &json!({"key": "mcp_token", "value": "x"})).unwrap_err().contains("only be changed by the person"));
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
        // A folder stands in for the Recycle Bin.
        ws.deleted = Some(crate::recycle::Pending::new(crate::recycle::MCP_UNDO_FOR, std::sync::Arc::new(crate::recycle::tests::FolderBin::new(dir.join("bin")))));
        ws.call("library_delete", &json!({"path": path})).unwrap();
        assert!(!path.exists() && !path.with_extension("json").exists());
        assert!(library::scan(&lib).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleted_library_pictures_come_back_with_undo_delete() {
        let (dir, mut ws) = fixed("undo-delete");
        let bin = std::sync::Arc::new(crate::recycle::tests::FolderBin::new(dir.join("bin")));
        ws.deleted = Some(crate::recycle::Pending::new(crate::recycle::MCP_UNDO_FOR, bin.clone()));
        assert!(ws.call("undo_delete", &json!({})).unwrap_err().contains("nothing to undo"));
        ws.set(Doc::new(image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))), "pic").unwrap();
        ws.call("library_add", &json!({"title": "One", "tags": ["a"]})).unwrap();
        // Two captures in the same second get different names.
        ws.call("library_add", &json!({"title": "Two", "tags": ["b"]})).unwrap();
        let lib = dir.join("lib");
        let before: Vec<_> = library::scan(&lib).into_iter().map(|i| (i.path, i.meta.title, i.meta.tags)).collect();
        assert_eq!(before.len(), 2);
        let r = ws.call("library_delete", &json!({"path": before[0].0})).unwrap();
        let said = r[0]["text"].as_str().unwrap();
        assert!(said.contains("undo_delete") && said.contains("120 seconds"), "the answer says how to undo: {said}");
        assert_eq!(ws.structured.as_ref().unwrap()["undo"], "undo_delete");
        let r = ws.call("library_delete", &json!({"path": before[1].0})).unwrap();
        assert!(r[0]["text"].as_str().unwrap().contains("2 pictures"), "{r:?}");
        assert!(library::scan(&lib).is_empty());
        let in_bin = if crate::recycle::way_here() == crate::recycle::Way::Restore { 2 } else { 0 };
        assert_eq!(bin.holds(), in_bin, "in the Recycle Bin, or waiting beside it on the Mac and in the Flatpak");
        let r = ws.call("undo_delete", &json!({})).unwrap();
        assert!(r[0]["text"].as_str().unwrap().starts_with("Restored"), "{r:?}");
        assert_eq!(r.iter().filter(|c| c["type"] == "resource_link").count(), 2);
        let after: Vec<_> = library::scan(&lib).into_iter().map(|i| (i.path, i.meta.title, i.meta.tags)).collect();
        assert_eq!(after, before, "both back, with their titles and tags");
        assert_eq!(bin.holds(), 0);
        assert!(ws.call("undo_delete", &json!({})).unwrap_err().contains("nothing to undo"), "once only");
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
