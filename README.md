<div align="center">

<img src="assets/snagster-256.png" alt="" width="96">

# Snagster

**Capture your screen, mark it up, share it.**

Windows · Mac · Linux · works offline · written in Rust · free and open source

[**⬇️ Windows**](https://github.com/SoloSaftis/Snagster/releases/latest/download/Snagster-windows-x64-portable.zip) &nbsp;·&nbsp;
[**⬇️ Mac**](https://github.com/SoloSaftis/Snagster/releases/latest/download/Snagster-macos-arm64.zip) &nbsp;·&nbsp;
[**⬇️ Linux**](https://github.com/SoloSaftis/Snagster/releases/latest/download/Snagster-linux-x86_64.AppImage)

<img src="assets/screenshots/snagster.png" alt="Snagster: the tools on the left, the picture with arrows, steps and a callout in the middle, and the options on the right" width="900">

</div>

## What it does

- **Capture anything** – press Print Screen, then drag an area or click a window, button or menu. Or the whole screen, all screens, a fixed size, or fun shapes: circle, lasso, star, heart.
- **Long pages** – scrolling and sideways capture stitch a whole web page or document into one picture.
- **Mark it up** – arrows, boxes, text in any of your fonts, speech bubbles, numbered steps, 1,300+ color emojis, highlighter, spotlight, magnifier, blur and smart redact for private details.
- **Tidy it up** – crop, cut out, move parts of the picture, borders, shadows, torn edges and more.
- **Grab text** – copy the text out of any picture or part of the screen (Windows).
- **Record videos** of your screen as MP4 (H.264 or HEVC) or WMV, at anything from 1 to 120 frames per second. On Windows the graphics chip does the work (Intel Quick Sync, AMD or NVIDIA encoders), so recording barely touches the processor or the battery.
- **Record GIFs** that stay small, and step-by-step guides as PDF, Word or web pages.
- **Library** – every capture is kept, searchable by date, program and tags; works from your OneDrive or Dropbox folder.
- **Your own hotkeys**, presets and themes, a capture panel and a quick menu in the corner of the taskbar.
- **For AI apps** – Snagster is also an MCP server, so Claude, Cursor and other AI apps can capture and mark up pictures too.

**Your pictures stay on your computer.** No account, no ads, no tracking. Nothing is sent anywhere.

<div align="center">
<img src="assets/screenshots/capture-panel.png" alt="The capture panel: All-in-One, Image and Video, with selection, effects and share choices and a big Capture button" width="780">
</div>

## Download

| Your computer | Download | Then |
|---|---|---|
| **Windows** 10 or 11 | [⬇️ Download for Windows](https://github.com/SoloSaftis/Snagster/releases/latest/download/Snagster-windows-x64-portable.zip) | Open the folder and double-click **Snagster** |
| **Mac** with Apple chip (M1 or newer) | [⬇️ Download for Mac](https://github.com/SoloSaftis/Snagster/releases/latest/download/Snagster-macos-arm64.zip) | Open it and drag **Snagster** to Applications. Then allow it under System Settings → Privacy & Security → **Screen Recording** |
| **Linux** | [⬇️ Download for Linux](https://github.com/SoloSaftis/Snagster/releases/latest/download/Snagster-linux-x86_64.AppImage) | Right-click → Properties → allow running as a program, then double-click it ([folder version](https://github.com/SoloSaftis/Snagster/releases/latest/download/Snagster-linux-x86_64-portable.tar.gz)) |

<details>
<summary><b>Your computer warns you the first time?</b></summary>

The app is not signed yet, so you may see a warning once:

- **Windows** says "Windows protected your PC": click **More info**, then **Run anyway**.
- **Mac** says Apple "could not verify" Snagster: click **Done**, open System Settings → Privacy & Security, scroll down and click **Open Anyway** next to Snagster.

</details>

All versions and what changed: [Releases](https://github.com/SoloSaftis/Snagster/releases).

## For AI apps

Snagster is an MCP server: start it with `snagster --mcp`. In Snagster, **Settings → AI apps** shows
the exact line to copy. For Claude Code:

```sh
claude mcp add snagster -- "path/to/snagster" --mcp
```

The same tools work from a terminal: `snagster help` lists commands such as
`snagster capture region 0 0 800 600 -o shot.png`, `snagster record 10 --fps 60 -o clip.mp4` and
`snagster ocr picture.png`.

## For developers

Building, tests and how the code is laid out: [DEVELOPING.md](DEVELOPING.md).

## Licence

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. A contribution you submit is
licensed the same way.
