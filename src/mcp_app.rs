//! The MCP tools that make every option in Snagster reachable: the command catalogue
//! (`list_commands`), pressing any ribbon or menu command in the open window (`app_command`),
//! the typed settings list (`list_settings`), the editor tools that aren't marks
//! (`editor_apply`), saving to cloud folders, the GIF editor and Studio's scene and sound edits.
//! Each one runs the same code the window's buttons run.

use super::{Outcome, Tool, Workspace, index_arg, need, num, obj, parse_color, rect_arg, saved, str_of, text};
use crate::catalog::{self, Who};
use crate::doc::Doc;
use eframe::egui::pos2;
use serde_json::{Value, json};
use std::path::PathBuf;

pub(super) fn tools() -> Vec<Tool> {
    let n = |d: &str| json!({"type": "number", "description": d});
    let s = |d: &str| json!({"type": "string", "description": d});
    let ids: Vec<&str> = catalog::COMMANDS.iter().filter(|c| c.window).map(|c| c.id).collect();
    vec![
        Tool {
            name: "list_commands",
            description: "Every button, menu item and tick box in Snagster's window (ribbon tabs, menus, Stream, Studio): its id, where it is, its label, the MCP tool or setting that does the same, whether app_command can press it in the window, the value it takes, and its risk (safe, asks the person, reaches outside Snagster, deletes or ends something). Filter by text in the id, place or label.",
            schema: obj(json!({"filter": s("Only commands whose id, place or label contain this text.")}), &[]),
            edits: false,
        },
        Tool {
            name: "app_command",
            description: "Press a command in the open Snagster window, exactly as the person would (its ribbon button or menu item runs): zoom, panels, pins, Recent pictures, the library, GIF editor and timer windows, Steps recording, email, print, save dialogs, settings pages, Studio and streaming controls... It acts on the window's own pictures, not this server's current picture (open_in_snagster sends that one). Starts Snagster when it isn't running (unless start is false). Commands that open a dialog (save as, print, email, install an update, forget a stream key) leave the decision to the person; risky ones (exit, recycle, stop a stream) say so in list_commands. Returns once the window has the command (it runs on the window's next frame).",
            schema: obj(
                json!({
                    "command": {"type": "string", "enum": ids, "description": "The command's id from list_commands (only those it marks window: true; the others have their own tool or set_setting key, which list_commands names)."},
                    "value": s("What the command needs, when list_commands gives it a value (a shape, a size, a page, an effect, a path...)."),
                    "start": {"type": "boolean", "description": "Start Snagster's window when it isn't running (default true)."}
                }),
                &["command"],
            ),
            edits: false,
        },
        Tool {
            name: "list_settings",
            description: "Every setting: its key (nested ones as stream.fps, studio.width, panel.tab), current value, kind and range, what it does, and whether an AI app may change it (some reach outside Snagster; mcp_* only the person changes; a few Snagster keeps itself). Change one with set_setting.",
            schema: obj(json!({"filter": s("Only keys or descriptions containing this text.")}), &[]),
            edits: false,
        },
        Tool {
            name: "editor_apply",
            description: "The editor tools that change the picture itself, and mark edits, as the window's tools do them. action: cut_out (remove the band x, y, width, height and close the gap: a full-width band when it is wider than tall... as the Cut out tool), fill (flood-fill from x, y with color, tolerance 0-255, default 32), erase (rub out along points [[x, y], ...] with width), lift (lift the rectangle x, y, width, height out as a movable picture mark: the Selection tool), scale_mark (index, factor: Bigger is 1.15, Smaller 0.87), duplicate_mark (index; the copy is offset by dx, dy, default 16: Copy and Paste mark), new_image (a blank width x height picture in color, as File > New image).",
            schema: obj(
                json!({
                    "action": {"type": "string", "enum": ["cut_out", "fill", "erase", "lift", "scale_mark", "duplicate_mark", "new_image"], "description": "Which editor tool (the tool's description says what each one needs)."},
                    "x": n("Left, picture pixels."), "y": n("Top."), "width": n("Width."), "height": n("Height."),
                    "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "[[x, y], ...] for erase."},
                    "color": s("#RRGGBB or a name (fill and new_image; default the mark colour / white)."),
                    "tolerance": n("fill: how different a colour may be and still fill (0-255, default 32)."),
                    "index": n("The mark's index from list_marks."),
                    "factor": n("scale_mark: 0.1-10."),
                    "dx": n("duplicate_mark: offset right (default 16)."), "dy": n("duplicate_mark: offset down (default 16)."),
                    "name": s("new_image: its name.")
                }),
                &["action"],
            ),
            edits: true,
        },
        Tool {
            name: "save_to_cloud",
            description: "Save the current picture into a cloud folder on this computer (OneDrive, Dropbox, Google Drive or iCloud), in its Snagster folder, as Share > Save to the cloud does: the cloud program then uploads it, so this reaches outside Snagster. Without cloud, lists the cloud folders found.",
            schema: obj(json!({"cloud": s("The cloud folder's name, e.g. OneDrive.")}), &[]),
            edits: false,
        },
        Tool {
            name: "gif_edit",
            description: "The GIF editor: info (frames and size of a GIF), trim (keep frames first..last, counted from 0, into out), append (add other's frames to the end; written next to path as NAME_combined.gif), frame (make frame number frame the current picture), slideshow (a GIF of the pictures in paths, seconds each, into out). An existing out is replaced.",
            schema: obj(
                json!({
                    "action": {"type": "string", "enum": ["info", "trim", "append", "frame", "slideshow"], "description": "What to do (the tool's description says what each one needs)."},
                    "path": s("The GIF file (full path)."), "out": s("Where to write the new GIF (trim, slideshow); an existing file is replaced."), "other": s("append: the GIF to add."),
                    "first": n("trim: the first frame kept, from 0."), "last": n("trim: the last frame kept, from 0."), "frame": n("frame: which frame, from 0."),
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "slideshow: the pictures."},
                    "seconds": n("slideshow: seconds per picture (default the setting).")
                }),
                &["action"],
            ),
            edits: true,
        },
        Tool {
            name: "studio_edit",
            description: "Studio's scene and sound buttons: copy_scene (a copy of scene, made current), move_source (source one place: direction 1 in front, -1 behind), add_sound (input computer, microphone (id from list_audio_devices, \"\" the default), file (a WAV path) or program (name from list_sound_programs) to scene), remove_sound / share_sound (sound number from studio_scenes, shared true for the sounds in every scene; share moves it between every scene and this scene only), sound_volume (volume 0-2, muted).",
            schema: obj(
                json!({
                    "action": {"type": "string", "enum": ["copy_scene", "move_source", "add_sound", "remove_sound", "share_sound", "sound_volume"], "description": "Which Studio button (the tool's description says what each one needs)."},
                    "scene": n("Scene number from studio_scenes (default the current one)."),
                    "source": n("move_source: the source's number."), "direction": n("move_source: 1 in front, -1 behind."),
                    "input": {"type": "string", "enum": ["computer", "microphone", "file", "program"], "description": "add_sound: the computer's sound, a microphone, a WAV file or one program's sound."}, "id": s("add_sound microphone: its id."), "path": s("add_sound file: a WAV file."), "name": s("add_sound program: its name from list_sound_programs."),
                    "sound": n("The sound's number."), "shared": {"type": "boolean", "description": "The sound is one of those in every scene."},
                    "volume": n("sound_volume: 0-2 (1 is as recorded)."), "muted": {"type": "boolean", "description": "sound_volume: silence it (true) or not."}
                }),
                &["action"],
            ),
            edits: false,
        },
        Tool {
            name: "list_sound_programs",
            description: "The programs that have sound open now (name to use with set_program_sound, label people read, playing: sound right now where the system can tell, Windows), and whether this computer can capture one program's sound (Windows 10 2004+, Linux with PipeWire, macOS 13+ with the Screen Recording permission; reason says why not).",
            schema: obj(json!({}), &[]),
            edits: false,
        },
        Tool {
            name: "set_program_sound",
            description: "One program's sound in a Studio scene: its volume (0-2) and mute. A program not in the scene yet is added (to the scene's own sounds, or to every scene with shared true); its sound comes in whenever it runs. name is from list_sound_programs (Windows: the exe name, Linux: the program, macOS: the bundle id).",
            schema: obj(
                json!({
                    "name": s("The program, from list_sound_programs."), "label": s("What the mixer calls it when added (default: from the name)."),
                    "scene": n("Scene number from studio_scenes (default the current one)."), "shared": {"type": "boolean", "description": "When added: in every scene."},
                    "volume": n("0-2 (1 is as it plays)."), "muted": {"type": "boolean", "description": "Silence it (true) or not."}
                }),
                &["name"],
            ),
            edits: false,
        },
    ]
}

/// Starts Snagster's window (a new process) and waits up to `secs` for it to listen.
fn start_window(id: &str, value: Option<&str>, secs: u64) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).spawn().map_err(|e| format!("couldn't start Snagster: {e}"))?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(300));
        match crate::instance::send_command(id, value) {
            Ok(()) => return Ok(()),
            Err(e) if std::time::Instant::now() > until => return Err(format!("{e} (Snagster was started but didn't answer in {secs} s; is \"Allow more than one Snagster\" on?)")),
            Err(_) => {}
        }
    }
}

fn command_json(c: &catalog::Command) -> Value {
    json!({
        "id": c.id, "place": c.place, "labels": c.labels, "tool": c.tool, "setting": c.setting,
        "window": c.window, "value": c.value, "risk": c.risk.name()
    })
}

impl Workspace {
    pub(super) fn call_app(&mut self, name: &str, a: &Value) -> Outcome {
        match name {
            "list_commands" => {
                let f = str_of(a, "filter").unwrap_or("").to_lowercase();
                let mut all: Vec<Value> = catalog::COMMANDS.iter().map(command_json).collect();
                // The editor's tool bar: select_tool picks one in the window; these tools do its work here.
                for t in crate::doc::Tool::ALL {
                    let (tool, how) = catalog::editor_tool_route(t);
                    all.push(json!({"id": format!("tool:{t:?}"), "place": "Editor tool bar", "labels": [t.label()], "tool": tool, "setting": "", "window": false, "value": how, "risk": "safe"}));
                }
                for (cmd, tool) in catalog::CLI {
                    all.push(json!({"id": format!("cli:{cmd}"), "place": "Command line (snagster help)", "labels": [cmd], "tool": tool, "setting": "", "window": false, "value": "", "risk": "safe"}));
                }
                let has = |v: &Value, k: &str| v[k].as_str().is_some_and(|s| s.to_lowercase().contains(&f));
                let list: Vec<Value> = all
                    .into_iter()
                    .filter(|c| f.is_empty() || has(c, "id") || has(c, "place") || c["labels"].as_array().is_some_and(|l| l.iter().any(|l| l.as_str().is_some_and(|s| s.to_lowercase().contains(&f)))))
                    .collect();
                self.structured = Some(json!({"items": list}));
                Ok(vec![text(serde_json::to_string_pretty(&list).unwrap_or_default())])
            }
            "app_command" => {
                let id = str_of(a, "command").ok_or("missing command")?;
                let value = str_of(a, "value");
                // Checked here first, so a typo never starts Snagster.
                crate::app::commands_ui::parse(id, value)?;
                // Draw takes over the person's mouse on every screen: only they turn it on.
                if id == "studio_ink" {
                    return Err("studio_ink (Draw) is only for the person: it turns their mouse into a pen on every screen. ink_clear removes ink.".into());
                }
                let c = catalog::command(id).ok_or("unknown command")?;
                let sent = match crate::instance::send_command(id, value) {
                    Ok(()) => Ok(()),
                    Err(e) if a.get("start").and_then(Value::as_bool) == Some(false) => Err(e),
                    Err(_) => start_window(id, value, 20),
                };
                sent?;
                let note = match c.risk {
                    catalog::Risk::Safe => String::new(),
                    r => format!(" ({})", r.name()),
                };
                Ok(vec![text(format!("Snagster's window runs {id}{note}."))])
            }
            "list_settings" => {
                let f = str_of(a, "filter").unwrap_or("").to_lowercase();
                let cur = super::masked_settings(&self.settings())?;
                let list: Vec<Value> = catalog::SETTINGS
                    .iter()
                    .filter(|s| f.is_empty() || s.key.contains(&f) || s.help.to_lowercase().contains(&f))
                    .map(|s| {
                        let v = s.key.split('.').fold(&cur, |v, part| &v[part]).clone();
                        let who = match s.who {
                            Who::Anyone => "yes",
                            Who::Outside => "yes, but it reaches outside Snagster",
                            Who::Person => "no: only the person, in Settings",
                            Who::Internal => "no: Snagster keeps it",
                        };
                        json!({"key": s.key, "value": v, "kind": s.kind.describe(), "help": s.help, "ai_may_change": who})
                    })
                    .collect();
                self.structured = Some(json!({"items": list}));
                Ok(vec![text(serde_json::to_string_pretty(&list).unwrap_or_default())])
            }
            "editor_apply" => self.editor_apply(a),
            "save_to_cloud" => {
                let clouds = crate::settings::cloud_folders();
                let names: Vec<&str> = clouds.iter().map(|c| c.0.as_str()).collect();
                let Some(want) = str_of(a, "cloud") else {
                    return Ok(vec![text(if names.is_empty() { "No OneDrive, Dropbox, Google Drive or iCloud folder found on this computer.".to_string() } else { format!("Cloud folders: {}", names.join(", ")) })]);
                };
                let (_, folder) = clouds.iter().find(|c| c.0.eq_ignore_ascii_case(want)).ok_or(format!("no {want} folder here (found: {})", if names.is_empty() { "none".to_string() } else { names.join(", ") }))?;
                let title = self.name.clone();
                let d = self.doc()?.clone();
                let mut s = self.settings();
                let p = s.cloud_save_path(folder, &title);
                crate::export::save_doc(&d, &title, &p, &s)?;
                self.save_settings(&s);
                Ok(saved(&p, format!("Saved {} (the cloud program uploads it).", p.display())))
            }
            "gif_edit" => self.gif_edit(a),
            "studio_edit" => self.studio_edit(a),
            "list_sound_programs" => Ok(vec![text(serde_json::to_string_pretty(&sound_programs(crate::audio::program_support(), crate::audio::programs)).unwrap_or_default())]),
            "set_program_sound" => self.set_program_sound(a),
            other => Err(format!("unknown tool {other} (tools/list shows them all)")),
        }
    }

    fn editor_apply(&mut self, a: &Value) -> Outcome {
        let action = str_of(a, "action").ok_or("missing action")?;
        match action {
            "cut_out" => {
                let r = rect_arg(a)?;
                self.change(|d| d.cut_out(r).ok_or("that band is outside the picture".into()))?;
                let (w, h) = self.doc()?.image.dimensions();
                Ok(vec![text(format!("Cut out a band; the picture is now {w} x {h}."))])
            }
            "fill" => {
                let p = pos2(need(a, "x")? as f32, need(a, "y")? as f32);
                let color = match str_of(a, "color") {
                    Some(c) => parse_color(Some(c)),
                    None => {
                        let c = self.settings().color;
                        eframe::egui::Color32::from_rgb(c[0], c[1], c[2])
                    }
                };
                let tol = num(a, "tolerance").unwrap_or(32.0).clamp(0.0, 255.0) as u8;
                self.change(|d| d.flood_fill(p, color, tol).ok_or("that point is outside the picture".into()))?;
                Ok(vec![text("Filled.")])
            }
            "erase" => {
                let pts = super::points_arg(a, "points")?.ok_or("erase needs points")?;
                let w = num(a, "width").unwrap_or(self.settings().width as f64).clamp(1.0, 200.0) as f32;
                // The window's eraser is three times the line width.
                self.change(|d| d.erase(&pts, w * 3.0).ok_or("nothing to erase there".into()))?;
                Ok(vec![text("Erased.")])
            }
            "lift" => {
                let r = rect_arg(a)?;
                self.change(|d| d.lift(r).ok_or("that area is outside the picture".into()))?;
                let n = self.doc()?.shapes.len();
                Ok(vec![text(format!("Lifted out as mark {}; move it with update_mark.", n - 1))])
            }
            "scale_mark" | "duplicate_mark" => {
                let i = index_arg(a, "index")?;
                let factor = num(a, "factor").unwrap_or(1.15).clamp(0.1, 10.0) as f32;
                let (dx, dy) = (num(a, "dx").unwrap_or(16.0) as f32, num(a, "dy").unwrap_or(16.0) as f32);
                self.change(|d| {
                    let mut d = d.clone();
                    let s = d.shapes.get(i).ok_or(format!("there's no mark {i} (see list_marks)"))?.clone();
                    if action == "scale_mark" {
                        d.shapes[i].scale(factor);
                    } else {
                        let mut copy = s;
                        copy.translate(eframe::egui::vec2(dx, dy));
                        d.shapes.push(copy);
                    }
                    Ok(d)
                })?;
                let n = self.doc()?.shapes.len();
                Ok(vec![text(if action == "scale_mark" { format!("Mark {i} is now {factor} times as big.") } else { format!("Copied mark {i} as mark {}.", n - 1) })])
            }
            "new_image" => {
                let (w, h) = (need(a, "width")?, need(a, "height")?);
                if !(1.0..=16384.0).contains(&w) || !(1.0..=16384.0).contains(&h) {
                    return Err("width and height are 1-16384".into());
                }
                let c = parse_color(str_of(a, "color").or(Some("white")));
                let img = image::RgbaImage::from_pixel(w as u32, h as u32, image::Rgba(c.to_array()));
                let name = str_of(a, "name").unwrap_or("New image").to_string();
                self.set(Doc::new(img), &name)
            }
            other => Err(format!("{other} isn't an editor_apply action")),
        }
    }

    fn gif_edit(&mut self, a: &Value) -> Outcome {
        let action = str_of(a, "action").ok_or("missing action")?;
        let path = || str_of(a, "path").map(PathBuf::from).ok_or("missing path".to_string());
        let out = || str_of(a, "out").map(PathBuf::from).ok_or("missing out".to_string());
        match action {
            "info" => {
                let frames = crate::export::gif_frames(&path()?, 0)?;
                let ms: u64 = frames.iter().map(|f| f.1 as u64).sum();
                let (w, h) = frames.first().map(|f| f.0.dimensions()).unwrap_or((0, 0));
                Ok(vec![text(format!("{} frames, {:.1} s, {w} x {h}.", frames.len(), ms as f64 / 1000.0))])
            }
            "trim" => {
                let (first, last) = (index_arg(a, "first")?, index_arg(a, "last")?);
                let o = out()?;
                let n = crate::export::trim_gif(&path()?, first, last, &o)?;
                Ok(saved(&o, format!("Kept {n} frames in {}", o.display())))
            }
            "append" => {
                let other = str_of(a, "other").map(PathBuf::from).ok_or("missing other")?;
                let o = crate::export::combine_gifs(&path()?, &other)?;
                Ok(saved(&o, format!("Combined into {}", o.display())))
            }
            "frame" => {
                let i = index_arg(a, "frame")?;
                let frames = crate::export::gif_frames(&path()?, 0)?;
                let n = frames.len();
                let (img, _) = frames.into_iter().nth(i).ok_or(format!("the GIF has {n} frames (0-{})", n.saturating_sub(1)))?;
                self.set(Doc::new(img), &format!("Frame {}", i + 1))
            }
            "slideshow" => {
                let paths: Vec<String> = a.get("paths").and_then(Value::as_array).map(|v| v.iter().filter_map(|p| p.as_str().map(String::from)).collect()).unwrap_or_default();
                if paths.len() < 2 {
                    return Err("a slideshow needs two pictures or more in paths".into());
                }
                let imgs = paths.iter().map(|p| crate::export::open_picture(std::path::Path::new(p))).collect::<Result<Vec<_>, _>>()?;
                let s = self.settings();
                let secs = num(a, "seconds").map(|v| v as f32).unwrap_or(s.slideshow_s);
                let o = out()?;
                let n = crate::export::slideshow_gif(&imgs, s.gif_max_width, secs, &o)?;
                Ok(saved(&o, format!("Made a {n}-picture slideshow: {}", o.display())))
            }
            other => Err(format!("{other} isn't a gif_edit action")),
        }
    }

    fn studio_edit(&mut self, a: &Value) -> Outcome {
        use crate::app::studio_ui;
        let action = str_of(a, "action").ok_or("missing action")?;
        let mut s = self.settings();
        let st = &mut s.studio;
        let scene = match num(a, "scene") {
            Some(_) => index_arg(a, "scene")?,
            None => st.current.min(st.scenes.len().saturating_sub(1)),
        };
        if scene >= st.scenes.len() {
            return Err(format!("there's no scene {scene} (see studio_scenes)"));
        }
        let msg = match action {
            "copy_scene" => {
                st.set_current(scene)?;
                studio_ui::copy_scene(st);
                format!("Copied scene {scene} as scene {} (now the current one).", st.current)
            }
            "move_source" => {
                let i = index_arg(a, "source")?;
                let d = need(a, "direction")? as i32;
                if !studio_ui::move_item(&mut st.scenes[scene].items, i, d) {
                    return Err("that source can't move that way".into());
                }
                format!("Moved source {i} {}.", if d > 0 { "in front" } else { "behind" })
            }
            "add_sound" => {
                use crate::audio::SoundInput;
                let (input, name) = match str_of(a, "input").ok_or("missing input")? {
                    "computer" => (SoundInput::Computer, "Computer sound".to_string()),
                    "microphone" => (SoundInput::Microphone { id: str_of(a, "id").unwrap_or("").to_string() }, "Microphone".to_string()),
                    "file" => {
                        let p = str_of(a, "path").ok_or("a sound file needs path")?;
                        if !std::path::Path::new(p).is_file() {
                            return Err(format!("there's no file {p}"));
                        }
                        (SoundInput::File { path: p.to_string() }, std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                    }
                    "program" => {
                        let raw = str_of(a, "name").ok_or("a program needs name (see list_sound_programs)")?;
                        let name = crate::audio::program_key(raw);
                        if name.is_empty() {
                            return Err("a program needs name (see list_sound_programs)".into());
                        }
                        let label = str_of(a, "label").map(str::trim).filter(|l| !l.is_empty()).map(String::from).unwrap_or_else(|| crate::audio::program_label(&name));
                        (SoundInput::Program { name }, label)
                    }
                    other => return Err(format!("{other} isn't computer, microphone, file or program")),
                };
                studio_ui::add_sound_to(st, scene, crate::studio::SceneSound::new(input, &name))?;
                format!("Added {name} to scene {scene}.")
            }
            "remove_sound" | "share_sound" => {
                let i = index_arg(a, "sound")?;
                let shared = a.get("shared").and_then(Value::as_bool).unwrap_or(false);
                let count = if shared { st.shared_sounds.len() } else { st.scenes[scene].sounds.len() };
                if i >= count {
                    return Err(format!("there's no sound {i} there (see studio_scenes)"));
                }
                let ask = if action == "remove_sound" { studio_ui::RowAsk::Remove } else { studio_ui::RowAsk::Share };
                studio_ui::sound_change(st, scene, shared, i, ask)?;
                if action == "remove_sound" { format!("Removed sound {i}.") } else { format!("Moved sound {i} {}.", if shared { "to this scene only" } else { "to every scene" }) }
            }
            "sound_volume" => {
                let i = index_arg(a, "sound")?;
                let shared = a.get("shared").and_then(Value::as_bool).unwrap_or(false);
                let list = if shared { &mut st.shared_sounds } else { &mut st.scenes[scene].sounds };
                let snd = list.get_mut(i).ok_or(format!("there's no sound {i} there (see studio_scenes)"))?;
                if let Some(v) = num(a, "volume") {
                    snd.volume = v.clamp(0.0, 2.0) as f32;
                }
                if let Some(m) = a.get("muted").and_then(Value::as_bool) {
                    snd.muted = m;
                }
                format!("Sound {i}: volume {:.2}{}.", snd.volume, if snd.muted { ", muted" } else { "" })
            }
            other => return Err(format!("{other} isn't a studio_edit action")),
        };
        self.save_settings(&s);
        Ok(vec![text(msg)])
    }

    fn set_program_sound(&mut self, a: &Value) -> Outcome {
        use crate::audio::SoundInput;
        let raw = str_of(a, "name").ok_or("missing name (see list_sound_programs)")?;
        let name = crate::audio::program_key(raw);
        if name.is_empty() {
            return Err("missing name (see list_sound_programs)".into());
        }
        let mut s = self.settings();
        let st = &mut s.studio;
        let scene = match num(a, "scene") {
            Some(_) => index_arg(a, "scene")?,
            None => st.current.min(st.scenes.len().saturating_sub(1)),
        };
        if scene >= st.scenes.len() {
            return Err(format!("there's no scene {scene} (see studio_scenes)"));
        }
        let input = SoundInput::Program { name: name.clone() };
        let mut added = false;
        if !st.scene_sounds(scene).iter().any(|x| x.input == input) {
            let label = str_of(a, "label").map(str::trim).filter(|l| !l.is_empty()).map(String::from).unwrap_or_else(|| crate::audio::program_label(&name));
            crate::app::studio_ui::add_sound_to(st, scene, crate::studio::SceneSound::new(input.clone(), &label))?;
            if a.get("shared").and_then(Value::as_bool).unwrap_or(false) {
                let i = st.scenes[scene].sounds.len() - 1;
                crate::app::studio_ui::sound_change(st, scene, false, i, crate::app::studio_ui::RowAsk::Share)?;
            }
            added = true;
        }
        let own = st.scenes[scene].sounds.iter_mut();
        let snd = st.shared_sounds.iter_mut().chain(own).find(|x| x.input == input).ok_or("the program couldn't be added")?;
        if let Some(v) = num(a, "volume") {
            snd.volume = v.clamp(0.0, 2.0) as f32;
        }
        if let Some(m) = a.get("muted").and_then(Value::as_bool) {
            snd.muted = m;
        }
        let msg = format!("{}{}: volume {:.2}{}.", if added { "Added " } else { "" }, snd.name, snd.volume, if snd.muted { ", muted" } else { "" });
        self.save_settings(&s);
        Ok(vec![text(msg)])
    }
}

/// list_sound_programs' answer: whether one program's sound can be captured here (and why
/// not), and the programs with sound open.
fn sound_programs(support: Result<(), crate::audio::Unsupported>, list: fn() -> Vec<crate::audio::Program>) -> Value {
    match support {
        Ok(()) => json!({"supported": true, "programs": list()}),
        Err(why) => json!({"supported": false, "reason": why.code(), "message": why.text(), "fix": why.command(), "programs": []}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> Workspace {
        let mut w = Workspace::with_settings(crate::settings::Settings::default());
        w.set(Doc::new(image::RgbaImage::from_pixel(100, 60, image::Rgba([255, 255, 255, 255]))), "t").unwrap();
        w
    }

    fn txt(o: Outcome) -> String {
        o.unwrap()[0]["text"].as_str().unwrap().to_string()
    }

    #[test]
    fn list_commands_and_settings_come_from_the_catalogue() {
        let mut w = ws();
        let all: Vec<Value> = serde_json::from_str(&txt(w.call("list_commands", &json!({})))).unwrap();
        assert_eq!(all.len(), catalog::COMMANDS.len() + crate::doc::Tool::ALL.len() + catalog::CLI.len());
        assert!(all.iter().any(|c| c["id"] == "tool:CutOut" && c["tool"] == "editor_apply"));
        assert!(all.iter().any(|c| c["id"] == "cli:record" && c["tool"] == "record_video"));
        let zoom: Vec<Value> = serde_json::from_str(&txt(w.call("list_commands", &json!({"filter": "zoom"})))).unwrap();
        assert!(zoom.iter().all(|c| c["id"].as_str().unwrap().contains("zoom") || c["place"].as_str().unwrap().to_lowercase().contains("zoom")));
        assert!(zoom.iter().any(|c| c["id"] == "zoom_in" && c["window"] == true));
        let set: Vec<Value> = serde_json::from_str(&txt(w.call("list_settings", &json!({"filter": "stream.fps"})))).unwrap();
        assert_eq!(set[0]["value"], 30);
        assert_eq!(set[0]["kind"], "a whole number 1-120");
        let tok: Vec<Value> = serde_json::from_str(&txt(w.call("list_settings", &json!({"filter": "mcp_token"})))).unwrap();
        assert!(tok[0]["ai_may_change"].as_str().unwrap().starts_with("no"));
    }

    #[test]
    fn app_command_checks_before_it_reaches_the_window() {
        let mut w = ws();
        assert!(w.call("app_command", &json!({"command": "nope", "start": false})).unwrap_err().contains("list_commands"));
        assert!(w.call("app_command", &json!({"command": "capture_fixed_size", "start": false})).unwrap_err().contains("WIDTHxHEIGHT"));
        assert!(w.call("app_command", &json!({"command": "after_copy", "start": false})).unwrap_err().contains("set_setting"));
        assert_eq!(w.call("app_command", &json!({})).unwrap_err(), "app_command needs \"command\": The command's id from list_commands (only those it marks window: true; the others have their own tool or set_setting key, which list_commands names).");
        assert!(w.call("app_command", &json!({"command": "studio_ink", "start": false})).unwrap_err().contains("only for the person"));
    }

    #[test]
    fn editor_apply_uses_the_editor_tools() {
        let mut w = ws();
        assert!(txt(w.call("editor_apply", &json!({"action": "cut_out", "x": 0, "y": 10, "width": 100, "height": 20}))).contains("100 x 40"));
        w.call("undo", &json!({})).unwrap();
        txt(w.call("editor_apply", &json!({"action": "fill", "x": 5, "y": 5, "color": "#00FF00"})));
        assert_eq!(w.doc.as_ref().unwrap().image.get_pixel(50, 50).0, [0, 255, 0, 255]);
        txt(w.call("editor_apply", &json!({"action": "erase", "points": [[10, 10], [40, 10]], "width": 4})));
        assert!(txt(w.call("editor_apply", &json!({"action": "lift", "x": 0, "y": 0, "width": 20, "height": 20}))).contains("mark 0"));
        txt(w.call("editor_apply", &json!({"action": "duplicate_mark", "index": 0})));
        assert_eq!(w.doc.as_ref().unwrap().shapes.len(), 2);
        let before = w.doc.as_ref().unwrap().shapes[1].bounds().width();
        txt(w.call("editor_apply", &json!({"action": "scale_mark", "index": 1, "factor": 2})));
        assert!(w.doc.as_ref().unwrap().shapes[1].bounds().width() > before * 1.5);
        assert!(w.call("editor_apply", &json!({"action": "scale_mark", "index": 9})).unwrap_err().contains("no mark 9"));
        assert!(w.call("editor_apply", &json!({"action": "fill", "x": 500, "y": 5})).is_err());
        txt(w.call("editor_apply", &json!({"action": "new_image", "width": 30, "height": 20, "color": "black"})));
        assert_eq!(w.doc.as_ref().unwrap().image.dimensions(), (30, 20));
        assert!(w.call("editor_apply", &json!({"action": "new_image", "width": 0, "height": 20})).is_err());
        assert!(w.call("editor_apply", &json!({"action": "melt"})).is_err());
    }

    #[test]
    fn gif_edit_trims_appends_grabs_and_makes_slideshows() {
        let dir = std::env::temp_dir().join(format!("snagster-mcp-gif-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let frames: Vec<(image::RgbaImage, u32)> = (0..3u8).map(|i| (image::RgbaImage::from_pixel(8, 6, image::Rgba([i * 80, 0, 0, 255])), 100)).collect();
        let g = dir.join("g.gif");
        crate::export::write_gif(&g, &frames).unwrap();
        let mut w = ws();
        assert!(txt(w.call("gif_edit", &json!({"action": "info", "path": g}))).starts_with("3 frames"));
        let t = dir.join("t.gif");
        assert!(txt(w.call("gif_edit", &json!({"action": "trim", "path": g, "first": 1, "last": 2, "out": t}))).contains("Kept 2"));
        assert!(txt(w.call("gif_edit", &json!({"action": "append", "path": g, "other": t}))).contains("g_combined.gif"));
        txt(w.call("gif_edit", &json!({"action": "frame", "path": g, "frame": 2})));
        assert_eq!(w.doc.as_ref().unwrap().image.dimensions(), (8, 6));
        assert!(w.call("gif_edit", &json!({"action": "frame", "path": g, "frame": 7})).unwrap_err().contains("3 frames"));
        let p1 = dir.join("1.png");
        image::RgbaImage::new(10, 10).save(&p1).unwrap();
        let s = dir.join("s.gif");
        assert!(txt(w.call("gif_edit", &json!({"action": "slideshow", "paths": [p1, p1], "out": s, "seconds": 1}))).contains("2-picture"));
        assert!(w.call("gif_edit", &json!({"action": "slideshow", "paths": [p1], "out": s})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn studio_edit_uses_studios_scene_and_sound_rules() {
        let mut w = ws();
        txt(w.call("studio_add_source", &json!({"kind": "color", "color": "red"})));
        txt(w.call("studio_add_source", &json!({"kind": "text", "text": "hi"})));
        assert!(txt(w.call("studio_edit", &json!({"action": "copy_scene"}))).contains("now the current one"));
        let st = w.settings().studio;
        assert_eq!(st.scenes.len(), 2);
        assert_eq!(st.current, 1);
        let n = st.scenes[1].items.len();
        assert!(n >= 2);
        let last = st.scenes[1].items[n - 1].name.clone();
        txt(w.call("studio_edit", &json!({"action": "move_source", "source": n - 1, "direction": -1})));
        assert_eq!(w.settings().studio.scenes[1].items[n - 2].name, last);
        assert!(w.call("studio_edit", &json!({"action": "move_source", "source": 0, "direction": -1})).is_err());
        assert!(w.call("studio_edit", &json!({"action": "add_sound", "input": "file", "path": "/no/such.wav"})).is_err());
        let own = w.settings().studio.scenes[1].sounds.len();
        txt(w.call("studio_edit", &json!({"action": "add_sound", "input": "microphone", "id": "test-mic"})));
        assert_eq!(w.settings().studio.scenes[1].sounds.len(), own + 1);
        assert!(w.call("studio_edit", &json!({"action": "add_sound", "input": "microphone", "id": "test-mic"})).unwrap_err().contains("already"));
        assert!(w.call("studio_edit", &json!({"action": "sound_volume", "sound": own, "volume": 5})).unwrap_err().contains("from 0 to 2"), "a volume past the limit is refused, naming it");
        assert!(txt(w.call("studio_edit", &json!({"action": "sound_volume", "sound": own, "volume": 2, "muted": true}))).contains("volume 2.00, muted"));
        txt(w.call("studio_edit", &json!({"action": "remove_sound", "sound": own})));
        assert_eq!(w.settings().studio.scenes[1].sounds.len(), own);
        assert!(w.call("studio_edit", &json!({"action": "copy_scene", "scene": 9})).unwrap_err().contains("no scene 9"));
    }

    #[test]
    fn save_to_cloud_without_a_name_lists_the_folders() {
        let mut w = ws();
        let t = txt(w.call("save_to_cloud", &json!({})));
        assert!(t.starts_with("Cloud folders") || t.starts_with("No OneDrive"));
        assert!(w.call("save_to_cloud", &json!({"cloud": "NoSuchCloudXyz"})).unwrap_err().contains("no NoSuchCloudXyz folder"));
    }

    /// A program is added by its name, kept by its key, and its volume and mute are set by name.
    #[test]
    fn program_sound_is_added_and_set_by_name() {
        use crate::audio::SoundInput;
        let mut w = ws();
        let spotify = SoundInput::Program { name: "spotify".into() };
        // Added the first time (by any spelling of its exe), with a readable name.
        assert_eq!(txt(w.call("set_program_sound", &json!({"name": "Spotify.exe", "volume": 0.5}))), "Added Spotify: volume 0.50.");
        let st = w.settings().studio;
        let snd = st.scene_sounds(0).into_iter().find(|s| s.input == spotify).cloned().unwrap();
        assert_eq!((snd.name.as_str(), snd.volume, snd.muted), ("Spotify", 0.5, false));
        // Then changed in place, never added twice; a volume out of range is refused.
        assert!(w.call("set_program_sound", &json!({"name": "spotify", "volume": 9})).is_err());
        assert_eq!(txt(w.call("set_program_sound", &json!({"name": "spotify", "volume": 2, "muted": true}))), "Spotify: volume 2.00, muted.");
        assert_eq!(w.settings().studio.scene_sounds(0).iter().filter(|s| s.input == spotify).count(), 1);
        // In every scene when asked, with the label given.
        txt(w.call("set_program_sound", &json!({"name": "com.example.game", "label": "The Game", "shared": true})));
        let st = w.settings().studio;
        assert!(st.shared_sounds.iter().any(|s| s.input == SoundInput::Program { name: "com.example.game".into() } && s.name == "The Game"));
        let e = w.call("set_program_sound", &json!({"name": "  "})).unwrap_err();
        assert!(e.contains("needs \"name"), "{e}");
        assert!(w.call("set_program_sound", &json!({"name": "x", "scene": 7})).unwrap_err().contains("no scene 7"));
        // studio_edit's add_sound takes a program too.
        assert!(txt(w.call("studio_edit", &json!({"action": "add_sound", "input": "program", "name": "VLC.exe"}))).contains("Added Vlc"));
        assert!(w.call("studio_edit", &json!({"action": "add_sound", "input": "program", "name": "vlc"})).unwrap_err().contains("already"));
        assert!(w.call("studio_edit", &json!({"action": "add_sound", "input": "program"})).unwrap_err().contains("needs name"));
    }

    #[test]
    fn the_program_list_says_why_when_it_cant_be_had() {
        use crate::audio::{Program, Unsupported};
        fn two() -> Vec<Program> {
            vec![Program { name: "spotify".into(), label: "Spotify".into(), playing: true }, Program { name: "zoom".into(), label: "Zoom".into(), playing: false }]
        }
        let ok = sound_programs(Ok(()), two);
        assert_eq!(ok["supported"], true);
        assert_eq!(ok["programs"][0]["name"], "spotify");
        assert_eq!(ok["programs"][1]["playing"], false);
        let no = sound_programs(Err(Unsupported::Flatpak), two);
        assert_eq!((no["supported"].clone(), no["reason"].clone()), (json!(false), json!("flatpak_sandbox")));
        assert!(no["fix"].as_str().unwrap().starts_with("flatpak override"));
        assert_eq!(no["programs"], json!([]), "nothing listed when it can't be captured");
        assert_eq!(sound_programs(Err(Unsupported::OldWindows), two)["fix"], Value::Null);
        // The real call answers in that shape on every system.
        let v: Value = serde_json::from_str(&txt(ws().call("list_sound_programs", &json!({})))).unwrap();
        assert!(v["supported"].is_boolean() && v["programs"].is_array(), "{v}");
    }
}
