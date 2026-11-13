# Snagster

Capture your screen, mark it up, share it. Written in Rust (egui).

## Features

- **Capture**: all-in-one picker (drag an area, click a window, button or menu), full screen,
  all screens, window list, window in front, repeat last area, fixed size, fun shapes (circle,
  freehand lasso, polygon, star, heart, triangle), several areas at once, menus with sub-menus,
  scrolling and panoramic capture (automatic or while you scroll), delay, schedule, interval.
- **Global hotkeys** you can change, per-preset hotkeys, and taking over keys another program
  already holds (Windows).
- **Capture panel** and a **tray icon** with a quick menu.
- **Editor**: arrow, line, box, oval, pen, marker, text (any installed font), callout, step
  numbers or letters, color emoji stamps, magnify, spotlight, blur / pixelate / smart redact,
  cut out, crop, selection (move part of the picture), fill, eraser, movable mouse pointer,
  shadows, quick styles, themes, undo and redo.
- **Image effects**: border, drop shadow, rounded corners, torn edge, perspective, color
  adjustment, sharpen, soften, grayscale, invert, sepia, watermark, capture info, resize, trim,
  canvas size, rotate, flip.
- **Text recognition** (Windows): grab text, smart redact, simplify.
- **GIF recording** that skips unchanged frames and stores only what changed, plus trimming,
  frame grabbing, combining and slideshows.
- **Library** with search, tags, program and month grouping; works from a OneDrive / Dropbox
  folder.
- **Saving**: PNG, JPEG, BMP, GIF, WebP, TIFF, PDF, SVG, Word guides, web page guides and
  `.snag` projects that keep marks editable.

## Download

Builds for Windows, Linux and macOS are made by GitHub Actions on every push (see the
*Actions* tab) and attached to each release.

On macOS, allow Snagster under *System Settings > Privacy & Security > Screen Recording*. The
macOS build is not signed, so the first start needs right-click > Open.

## Build it yourself

```sh
cargo build --release
```

Linux needs the development packages for PipeWire, Wayland, XCB, GBM, EGL and clang
(see `.github/workflows/build.yml`).

## Test options

| Option | What it does |
|---|---|
| `--ui-test` | runs every button's action in the real window and reports |
| `--self-test` | capture, fonts, emojis, text recognition and every file format |
| `--hotkey-test` | presses each hotkey for real (Windows) |
| `--record-test` / `--record-bench` | cost of GIF recording |
| `--screenshot <file.png>` | starts, draws, saves a picture of the window and quits |
