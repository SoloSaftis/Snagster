# Snagster in the Microsoft Store

A separate Store edition, next to the GitHub downloads. The Store signs the package itself, so
people who install Snagster from the Store get no SmartScreen or "unknown publisher" warning,
and company laptops that allow Store apps (but block other installers) can have it. The Store
keeps it up to date: a Store-installed Snagster doesn't download updates itself
(`src/packaged.rs` `from_store`, `src/update.rs` `managed_by`). Settings, crash.log and captures
are in the same places as with the GitHub copies.

It is the same package as the GitHub MSIX (`AppxManifest.xml`), with the identity Partner Center
gives Snagster instead of `SoloSaftis.Snagster`, and it is uploaded unsigned.

## Once: the account and the name (the person, by hand)

1. Register as an individual developer at <https://storedeveloper.microsoft.com> ("Get started
   for free", Individual). It is free for individuals since 2025; it needs a personal Microsoft
   account and an ID check (government ID and a selfie). Start from that page: Partner Center's
   own sign-up is the old, paid flow.
2. In Partner Center: **Apps and games > New product > MSIX or PWA app**, and reserve the name
   **Snagster**.
3. Open **Product management > Product identity** and copy three values into the GitHub
   repository (Settings > Secrets and variables > Actions > Variables):
   - `STORE_IDENTITY_NAME`: *Package/Identity/Name* (looks like `12345SoloSaftis.Snagster`)
   - `STORE_PUBLISHER`: *Package/Identity/Publisher* (looks like `CN=1A2B3C4D-...`)
   - `STORE_PUBLISHER_DISPLAY_NAME`: *Package/Properties/PublisherDisplayName*

From then on every CI run has a `windows-store` artifact with
`Snagster-windows-x64-store.msix`, ready to upload. (Without the variables CI still makes the
package with stand-in values, so the step stays tested, but doesn't keep it.)

## Each Store release

1. Tag the release as usual (`vX.Y.Z`); wait for the green run.
2. Download the `windows-store` artifact of the tag's run.
3. Partner Center > Snagster > **Start update** (the first time: **Start submission**):
   - **Packages**: upload `Snagster-windows-x64-store.msix`. The version must be higher than
     the last one in the Store.
   - The first time also: **Pricing** (Free, all markets), **Properties** (category *Photo &
     video* or *Productivity*; privacy policy URL below), **Age ratings** (the questionnaire),
     and **Store listing** (description, at least one screenshot: the README pictures in
     `assets/` work; the 300 x 300 logo from `snagster --icon-png logo.png 300`).
   - **Submission options > restricted capabilities**: explain the two the package declares
     (text below).
4. Submit. Certification usually takes one to three business days.

### Privacy policy URL

`https://github.com/SoloSaftis/Snagster/blob/main/PRIVACY.md`

### Why the package needs its restricted capabilities (for the submission form)

- **runFullTrust**: Snagster is a desktop program (Rust, Win32). It captures the screen with
  Desktop Duplication and GDI, records video with Media Foundation, registers global hotkeys and
  a notification-area icon, and runs an MCP server for AI apps on 127.0.0.1.
- **unvirtualizedResources**: Snagster is also offered as a portable program. Turning off
  AppData virtualisation keeps its settings, crash report and captures in
  `%APPDATA%\Snagster`, Pictures and Videos for both editions, and lets the person open
  crash.log and the settings folder in Explorer and Notepad from Help > Problem reports. Without
  it the files would be in a private per-package folder that other programs can't see.

If certification refuses `unvirtualizedResources`, the Store edition can leave out the two
`desktop6:...Virtualization` lines and that capability; Snagster then keeps its files in the
package's private AppData, and Help > Problem reports (which opens the folder in Explorer) would
need to open the package's real folder instead.

## Later (not done)

- Submitting from CI with the Microsoft Store Developer CLI (`msstore`, MIT) and a Partner Center
  API key, instead of uploading by hand.
- A Store badge and link in `README.md` once the listing is live.
