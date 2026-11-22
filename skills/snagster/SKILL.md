---
name: snagster
description: Capture the screen, mark up and redact screenshots, read text on screen, record screen videos and GIFs, make step-by-step guides, and run or watch a live stream, through Snagster's MCP server on Windows, Linux or macOS. Use when a task needs a picture or video of this computer's screen (to check a UI, document a bug, show a result, write a how-to), needs private details hidden in a screenshot, or needs Snagster's recording, Studio, replay buffer or streaming driven by an agent.
license: MIT OR Apache-2.0
---

# Snagster for AI coding agents

Snagster is a free screen capture, recording and streaming app (MIT/Apache-2.0) with an MCP
server built in. Through it an agent can see this computer's screen, make clear annotated
screenshots and videos for the person, and change Snagster's own settings. Everything stays on
this computer: only `stream_start` (and the stream it runs), `check_for_update` and `app_command`'s
email and update buttons go online, and `save_to_cloud` puts a file where a cloud program uploads
it; their descriptions say so.

## When to use this skill

- You changed a user interface and want to **see it**: capture the window, look at it with
  `view_picture`, and fix what you see.
- The person wants a **screenshot with arrows, boxes, steps or text**, or one with **private details
  hidden** (emails, phone numbers, card numbers, addresses).
- You need **text that is on the screen** (an error dialog, a log in another program).
- The person wants a **screen video or GIF** of something (a bug, a demo), a **step-by-step guide**
  (PDF, Word, web page), or a **live stream** started, watched or stopped.
- Not for: reading files you can open yourself, or web pages you can fetch directly.

## Setup

Get Snagster from https://github.com/SoloSaftis/Snagster/releases (or the Microsoft Store /
Flathub where available). In Snagster, **Settings → AI apps** shows every line below with the
right path for this computer filled in; copy it from there when you can.

### Apps that start Snagster themselves (stdio)

Snagster serves MCP over stdin/stdout when started with `--mcp`.

Claude Code:

```sh
claude mcp add snagster -- "/path/to/snagster" --mcp
```

Claude Desktop, Cursor, VS Code, Windsurf, Zed, Cline, Continue, LM Studio and others:

```json
{ "mcpServers": { "snagster": { "command": "/path/to/snagster", "args": ["--mcp"] } } }
```

Where the program is:

| Download | Command |
|---|---|
| Windows portable zip | `C:\path\to\Snagster\Snagster.exe` |
| Windows (MSIX / Store) | `snagster` (the app's command alias) |
| macOS | `/Applications/Snagster.app/Contents/MacOS/snagster` |
| Linux AppImage / tar.gz | `/path/to/Snagster-linux-x86_64.AppImage` or `/path/to/Snagster/Snagster` |
| Linux Flatpak | command `flatpak`, args `["run", "io.github.SoloSaftis.Snagster", "--mcp"]` |

### Apps that connect to a server (HTTP)

Switch on **Settings → AI apps → Let AI apps on this computer connect**, or run the server without
a window:

```sh
snagster mcp-http --port 45831
```

| Transport | Address |
|---|---|
| Streamable HTTP | `http://127.0.0.1:45831/mcp` |
| HTTP + SSE (older apps) | `http://127.0.0.1:45831/sse` |

The server listens only on this computer and refuses web pages from other sites. When the person
has set a password in Settings → AI apps, send it as `Authorization: Bearer <password>`. Only the
person can change the server's own settings (`mcp_http`, `mcp_port`, `mcp_token`).

### Check it works

Call `app_info`: it gives Snagster's version, the system, which features work here (text
recognition, sound recording, encoders) and its folders.

## How the tools work

- **One current picture.** Captures, `open_image`, `paste_from_clipboard` and `combine_images`
  make the current picture; marks, blur, crop, effects and `undo`/`redo` change it; `view_picture`
  shows it to you; `save`, `quick_save`, `copy_to_clipboard`, `library_add` or `open_in_snagster`
  keep it. Nothing is written to disk until you save.
- **Coordinates are pixels from the top-left.** Captures and recordings take *desktop* pixels
  (`list_monitors` gives each screen's position; screens left of or above the main one have
  negative coordinates). Marks, blur and crop take *picture* pixels of the current picture.
- **Marks** are numbered from 0 (`list_marks`); `update_mark` and `remove_mark` take that number.
- **Defaults come from the person's settings.** Leave an argument out to use them.
  `list_settings` explains every setting with its range; `set_setting` changes one (checked).
- **Arguments are checked first.** A wrong name, a number sent as text or a value out of range
  is refused with a message that names the argument, what was sent and what the tool takes. Read
  it and call again; don't guess.
- **Long calls** (recordings, scrolling captures, guides) report progress when the request has a
  progress token, and stop when cancelled.
- **Every button in the window** is in `list_commands`; `app_command` presses one in the open
  Snagster window, as the person would.

## Recipes

**See your UI change**

1. `list_windows` → find the window's id.
2. `capture_window` `{"id": 1234}` (or `{"title": "My App"}`).
3. `view_picture` `{"max_size": 1600}` → look at it.

**Annotated screenshot for a bug report**

1. `capture_region` `{"x": 0, "y": 0, "width": 1280, "height": 800}`
2. `add_mark` `{"kind": "arrow", "x": 900, "y": 120, "x2": 760, "y2": 300, "color": "red"}`
3. `add_mark` `{"kind": "callout", "x": 920, "y": 80, "x2": 760, "y2": 300, "text": "Button is cut off"}`
4. `add_mark` `{"kind": "step", "x": 200, "y": 400}` (numbers itself 1, 2, 3...)
5. `view_picture`, then `save` `{"path": "/abs/path/bug.png"}`

**Hide private details before sharing**

1. `capture_window` `{"title": "Mail"}`
2. `smart_redact` (Windows; finds emails, phone and card numbers, addresses, IP addresses)
3. `view_picture`; `blur_area` `{"x": ..., "y": ..., "width": ..., "height": ..., "mode": "pixelate"}`
   anything still visible
4. `copy_to_clipboard`

**Read text on the screen**

- `read_screen_text` `{"x": 0, "y": 0, "width": 800, "height": 600}` (no capture kept), or
  `read_text` for the current picture. Windows only.

**Record a short video or GIF**

- `record_video` `{"seconds": 10, "fps": 30, "audio": "system"}` → returns the video's path, length and
  the encoder used. Add `x`, `y`, `width`, `height` for an area or `monitor_id` for one screen.
- `record_gif` `{"seconds": 5, "x": 100, "y": 100, "width": 640, "height": 360}`
- `video_info` `{"path": "..."}` reads a recording back to check it.

**Step-by-step guide**

1. Capture each step and `save` it (or keep the last one with `include_current`).
2. `make_guide` `{"title": "Install the app", "steps": [{"path": "/abs/1.png", "caption": "Open Settings"}, {"path": "/abs/2.png", "caption": "Click Install"}], "path": "/abs/guide.pdf"}`
   (`.pdf`, `.html`, `.docx` or `.png`).

**Keep a moment after it happened**

- `replay_start` `{"seconds": 60}` keeps the last minute of the screen in memory;
  `replay_save` `{"seconds": 20}` writes the last 20 s as an MP4; `replay_stop` ends it.
  When the person has replay on in the Snagster window, these act on the window's buffer
  (`replay_status` says `where`), and the window saves everything it keeps.

**Studio sound and live ink**

- `list_sound_programs` lists the programs playing sound; `set_program_sound`
  `{"name": "spotify.exe", "volume": 0.8}` puts one in the current scene with its own volume or mute.
- `ink_clear` removes what the person drew with Studio's Draw (only the person draws; how long ink
  stays is the setting `studio.ink_mode`). Ink is in Studio's stream and video, not in replays.

**Live streaming (ask the person first)**

- `stream_services` lists Twitch, YouTube, Kick, Facebook, Restream and custom RTMP servers.
- `stream_start` `{"service": "youtube", "source": "screen"}` uses the stream key saved in Snagster
  (never shown back). Then `stream_status`, `stream_stats` (dropped frames, upload health,
  warnings) and `stream_stop`.

## Be careful with

The tool annotations say which tools only read (`readOnlyHint`), which may delete or replace
something (`destructiveHint`) and which reach outside this computer (`openWorldHint`).

- **Ask the person before** going live (`stream_start`), deleting from the library
  (`library_delete`; `undo_delete` brings it back for 2 minutes), saving over an existing file
  (`save`, `make_guide`, `gif_edit`), saving to a cloud folder (`save_to_cloud`) or changing
  settings that reach outside Snagster (folders, start-up, update checks).
- `app_command` presses real buttons in the person's window; `list_commands` gives each
  command's risk. Commands that print, email or install an update open a dialog and leave the
  decision to the person.
- Text recognition (`read_text`, `read_screen_text`, `smart_redact`, `simplify`) and sound in
  recordings work on Windows. On Wayland the desktop may ask the person to choose a screen.

## Prompts and resources

The server also offers prompts (`annotate_screenshot`, `redact_screenshot`, `record_demo`,
`step_guide`, `change_language`) and resources: `snagster://picture/current`,
`snagster://settings`, `snagster://library`, and the templates `snagster://settings/{key}`,
`snagster://library/{path}` and `snagster://languages/{code}`.

## From a terminal

The same tools work without an AI app: `snagster help` lists commands, and
`snagster call <tool> '<json arguments>'` runs any tool, e.g.
`snagster call capture_region '{"x":0,"y":0,"width":800,"height":600}'`.
