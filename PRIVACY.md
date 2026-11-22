# Snagster privacy policy

Snagster is a screen capture, recording and streaming program that runs on your own computer.
This policy covers every edition: the downloads on GitHub (Windows, macOS, Linux, Flatpak) and
the Microsoft Store edition.

## What Snagster collects

Nothing. Snagster has no accounts, no analytics, no telemetry and no advertising, and it sends
nobody your pictures, recordings, text or settings.

## What stays on your computer

- Your captures, recordings and projects, in the folders you choose (by default Pictures and
  Videos).
- Settings and a crash report (`crash.log`), in Snagster's settings folder (on Windows
  `%APPDATA%\Snagster`; the Microsoft Store edition keeps the files it creates in its own
  package folder, `%LOCALAPPDATA%\Packages\<package>\LocalCache\Roaming\Snagster`, which Windows
  removes when you uninstall it). The crash report never leaves your computer unless you send it to
  someone yourself.
- Live-streaming keys, encrypted for your user account (Windows), in your Keychain (macOS) or in
  a file only your user can read (Linux).
- Text recognition (OCR) runs on your computer with the system's own engine.

## When Snagster uses the network

- **Checking for updates** (once a day, and when you ask; you can turn it off in Settings):
  Snagster asks GitHub (`api.github.com`) about the newest release, and downloads it from
  GitHub when you choose to update. GitHub sees the request like any web request (your IP
  address and the Snagster version). The Microsoft Store edition doesn't do this; the Store
  updates it.
- **Live streaming**, only when you start a stream: the video goes to the services and servers
  you choose (for example Twitch or YouTube), under their own privacy policies.
- **AI apps (MCP)**: Snagster's MCP server listens only on your own computer (127.0.0.1) and only
  when you switch it on or an app starts it. What an AI app does with a picture it asked Snagster
  for is up to that app.

## Microphone and screen

Snagster sees your screen only when you capture, record or stream, and uses the microphone only
when you record or stream with microphone sound. Your system shows its own permission prompts
for these.

## Questions

Open an issue at <https://github.com/SoloSaftis/Snagster/issues>.

This policy was last changed on 2026-11-17.
