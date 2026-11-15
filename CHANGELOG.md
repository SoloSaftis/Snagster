# Changelog

What changed for people using Snagster, per version. Developer details are in `DEVLOG.md`.

## Unreleased (0.10.0)

New:

- **Choose the video encoder**: Settings > Recording or Streaming > "Video encoder" lists the
  encoders this computer has (Intel Quick Sync, NVIDIA, AMD, Apple's media engine, VA-API,
  software). *Automatic* stays the default. Choosing one that can't start never stops a
  recording or stream: Snagster uses the best one that works and says so.
- **A stream survives its encoder failing**: when the graphics chip's encoder stops working in
  the middle of a stream (a driver reset, a laptop switching chips), another encoder takes over
  within a frame and the stream goes on, instead of ending.
- **More responsive**: exporting several pictures, Word/PDF/web guides, combining pictures,
  templates, opening and importing files, Print/Email/"Open in another program" and the
  Recycle Bin now work in the background, so the window never stops answering ("Not
  responding") while they run. The Share tab no longer checks the cloud folders on every frame
  (a disconnected network drive could freeze it), "New from template" no longer redraws every
  picture while the menu is open, and "Start when Windows starts" no longer runs `reg.exe`.
- **Multistreaming**: go live on several services at once (say Twitch and YouTube) from one
  encode. Settings > Streaming > "Also stream to" takes up to four more services, each with its
  own server and stream key (kept as safely as the main key). The picture is sent at the size,
  frame rate and bitrate every chosen service takes. Each service connects, reconnects and drops
  frames on its own, so one slow or failing service doesn't affect the others, and the Stream tab
  shows how each one is doing. AI apps can multistream too (`stream_start` with `also`).
- **Live streaming** to Twitch, YouTube, Kick, Facebook Live, Restream or any RTMP/RTMPS server:
  the whole screen, a window (followed when it moves), an area or the Studio scene. A Stream tab
  in the ribbon and in the capture panel, a red LIVE badge with bitrate, dropped frames and
  time on the air, and Settings > Streaming (server, picture size, frame rate, bitrate,
  keyframes, graphics-chip encoding, sound, reconnecting). Snagster asks before going live the
  first time each run, and Stop is instant. It reconnects by itself when the connection drops.
  Optionally saves a copy of exactly what was streamed (FLV) in the recordings folder. Sound is
  streamed on Windows only for now. Not yet tried against the real services.
- **4K and 120 fps streams**: YouTube up to 3840 × 2160 at 60 fps (with YouTube's recommended
  bitrates, and HEVC by itself at 4K where the computer can encode it); custom servers up to 4K
  at 60 fps or 1440p at 120 fps. Settings > Streaming only offers what the service takes.
- **Streaming uses far less processor time** on Windows with a graphics-chip encoder and on
  macOS: the screen's pictures stay on the graphics chip from the copy to the encoder
  (about 4 % of one core instead of 114 % at 1080p30 on an Intel laptop, 18 % instead of 212 %
  at 1080p120). Other computers copy as before.
- **The stream key stays private**: encrypted for your Windows account, in the macOS Keychain,
  or in a file only you can read on Linux. It is never in the settings file and never shown in
  messages.
- **Snapping while you select**: the window, button, list or panel under the pointer is
  highlighted; click to take it, turn the mouse wheel to step out to the bigger part around it,
  hold Ctrl for the whole window. Windows reads controls with UI Automation; on a Mac allow
  Snagster under Privacy & Security > Accessibility.
- **Scrolling capture scrolls by itself**: it goes back to the top, scrolls, finds where the
  new part starts (sticky headers and footers kept once) and stops at the end. Where Snagster
  can't scroll other programs (Wayland, a Mac without the permission, Linux without xdotool),
  you scroll and press Stop. New settings: wheel notches per step, longest picture, start from
  the top.
- **Graphics-chip video encoding on Mac and Linux** (VideoToolbox, VA-API), with OpenH264 as the
  fallback; faster OpenH264 on several processor cores. Not yet tried on real Mac or Linux
  hardware.
- **Snagster's own AAC sound encoder**: the fallback when the system has none, and the
  groundwork for streams with sound on Linux once sound capture arrives there.
- **For AI apps**: 66 MCP tools (was 51 in 0.9.0), including going live, stream status and
  stopping; `update_mark`, `run_preset`, `simplify`, `quick_save`, `capture_last_area`, Studio
  scene tools, `library_tag` and `library_delete`. Long calls report progress; results come
  with structured data; resource templates and completions.
- **Snagster.exe has its own icon** in Explorer, Start, the taskbar and Alt+Tab.

Fixed:

- The window no longer freezes ("Not responding") while saving, scanning the library, opening
  GIFs, reading the clipboard or listing windows; that work happens in the background.
- Saving a very large still GIF is about 14 times faster (23 s to 1.7 s for 26 megapixels).
- Recording on a computer without speakers records the video without sound instead of failing.
- Redo while typing text could change the wrong mark.
- Smart redact labelled IP addresses as phone numbers.
- Norwegian shows as "Norsk bokmål" in the language list.
- AI apps: opening a new picture kept the old picture's redo steps; a negative number acted on
  the first mark, source or scene.
- Broken or hostile input (project files claiming huge pictures, damaged settings files, odd
  network data from a streaming server or an AI app) can no longer crash Snagster or make it
  reserve huge amounts of memory.
- The HTTP server for AI apps could stop accepting connections after many failed ones.

## 0.9.0 — 2026-11-14

- A Word-style ribbon (Home, Capture, Record, Studio, Edit, Image, Share, View, Help) with icons
  and a tooltip on every tool.
- 86 interface languages, picked from your computer, right to left for Arabic, Persian, Hebrew,
  Urdu and Uyghur.
- Videos with sound on Windows: the computer's sound, a microphone or both.
- Studio mode: scenes of screens, windows, areas, pictures, text and colours, with a mixer and
  recording.
- Automatic updates from GitHub (daily check, can be turned off).
- Pictures in Pictures/Snagster, videos in Videos/Snagster.
- An MCP server any AI app can use (51 tools), over stdio or HTTP, including local AI.
- A failing frame no longer closes the window; it is logged to crash.log.

## 0.8.0 — 2026-11-13

- Video recording to MP4 (H.264 or HEVC) or WMV at 1 to 120 frames per second, encoded by the
  graphics chip on Windows (Intel, AMD, NVIDIA), OpenH264 on Linux and macOS. Record menu,
  capture panel Video tab, Settings > Recording, pause and resume.

## 0.7.0 — 2026-11-13

- An MCP server for AI apps (22 tools) and command-line commands (capture, ocr, redact, effect,
  convert, call). Settings > AI apps. Downloads for Windows, macOS and Linux. MIT or
  Apache-2.0.

## 0.6.0 — 2026-11-13

- First release of Snagster (formerly Snag): capture areas, windows, menus, screens and shapes,
  scrolling capture, markup (arrows, boxes, text, callouts, steps, emojis, blur, spotlight,
  magnifier), effects, text recognition and smart redact on Windows, GIF and step recording,
  presets and hotkeys, tray icon and capture panel, the library, `.snag` projects, and
  PNG/JPEG/BMP/GIF/WebP/TIFF/PDF/SVG/Word output.
