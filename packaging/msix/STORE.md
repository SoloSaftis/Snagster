# Snagster in the Microsoft Store

A separate Store edition, next to the GitHub downloads. The Store signs the package itself, so
people who install Snagster from the Store get no SmartScreen or "unknown publisher" warning,
and company laptops that allow Store apps (but block other installers) can have it. The Store
keeps it up to date: a Store-installed Snagster doesn't download updates itself
(`src/packaged.rs` `from_store`, `src/update.rs` `managed_by`). Captures go to Pictures and
Videos as with the GitHub copies; settings and crash.log are in the package's own folder (below).

It is built from the same manifest as the GitHub MSIX (`AppxManifest.xml`), with the identity
Partner Center gives Snagster instead of `SoloSaftis.Snagster`, and it is uploaded unsigned.
`make-msix.ps1 -Store` (what CI uses for it) leaves out the manifest lines marked
`github-only`: the `unvirtualizedResources` capability and the two
`desktop6:...Virtualization` lines, which certification refused (Status, below).

### Where the Store edition keeps its files

Without `unvirtualizedResources`, Windows redirects the files a packaged program *creates* under
AppData to the package's own folder:
`%LOCALAPPDATA%\Packages\<package family name>\LocalCache\Roaming\Snagster` (settings.json,
crash.log, the stream key, the font cache). Snagster itself still sees them at
`%APPDATA%\Snagster`; Explorer and Notepad don't. So Help > Problem reports and the crash.log
line in Settings use `packaged::visible_path`, which finds the package's folder from
`GetCurrentPackageFamilyName`. Files that were already in the real `%APPDATA%\Snagster` (made by
a portable copy) are read and changed in place, so the Store edition starts with the portable
copy's settings but its own new files stay private. Pictures, Videos and any folder outside
AppData are never redirected. Uninstalling the Store edition removes its folder.

`snagster --paths` prints the package, settings.json, crash.log and the Problem reports folder
as other programs find them. CI installs a test-signed package with the Store's manifest
(`-Store`) on the Windows runner, saves a setting through it and checks that settings.json is in
the package's folder and that `--paths` and Problem reports name that folder, then runs the
button test from it.

## Once: the account and the name (done 2026-11-17: Store ID 9MVZGNTS547Z)

1. Register as an individual developer at <https://storedeveloper.microsoft.com> ("Get started
   for free", Individual). It is free for individuals since 2025; it needs a personal Microsoft
   account and an ID check (government ID and a selfie). Start from that page: Partner Center's
   own sign-up is the old, paid flow.
2. In Partner Center: **Apps and games > New product > MSIX or PWA app**, and reserve the name
   **Snagster**.
3. Open **Product management > Product identity** and copy three values into the GitHub
   repository (Settings > Secrets and variables > Actions > Variables):
   - `STORE_IDENTITY_NAME`: *Package/Identity/Name* (Snagster's is `SoloSaftis.Snagster`)
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
   - **Submission options > restricted capabilities**: explain the one the package declares,
     `runFullTrust` (text below).
4. Submit. Certification usually takes one to three business days.

### Privacy policy URL

`https://github.com/SoloSaftis/Snagster/blob/main/PRIVACY.md`

### Why the package needs its restricted capability (for the submission form)

- **runFullTrust**: Snagster is a desktop program (Rust, Win32). It captures the screen with
  Desktop Duplication and GDI, records video with Media Foundation, registers global hotkeys and
  a notification-area icon, and runs an MCP server for AI apps on 127.0.0.1.

The Store package declares no other restricted capability (the microphone is an ordinary
device capability, for recording with sound). It used to ask for `unvirtualizedResources` too,
to share `%APPDATA%\Snagster` with the portable copy; certification refused it (Status), so the
Store package leaves it out and keeps its files in the package's own folder (above). The GitHub
MSIX, which is never certified, keeps it.

## The listing in every language

`listing/<store-code>.json` holds the Store texts (description, features, captions, search
terms, release notes) in all 86 languages (AI-translated, like the interface packs).
`node packaging/msix/make-listing.js store-listing` (after
`snagster --icon-png store-listing/store-logo-300.png 300`) makes the folder to upload under
Store listings > Import listings > **Upload folder**: `listings.csv` plus the five screenshots
and the logo. `listing-fields.csv` is Partner Center's own export (its Field and ID columns must
not change). For a new version, change `ReleaseNotes` in every language, rebuild and import.

## Status

- 2026-11-18: submitted for certification (version 0.10.6.0, listing in all 86 languages,
  built from `main` after PR #2), with `runFullTrust` and `unvirtualizedResources`.
- Certification failed: "10.6.3 Capabilities": the request to use `unvirtualizedResources` was
  denied, with the advice to remove it and resubmit (read in Partner Center on 2026-11-22).
- 2026-11-22: the capability and the two virtualization lines removed from the Store package
  only (`make-msix.ps1 -Store`); Help > Problem reports opens the package's real folder. To be
  resubmitted by hand with version 0.10.8.0 or higher (Packages: replace
  `Snagster-windows-x64-store.msix`, then **Resubmit for certification**).

Store page: https://apps.microsoft.com/detail/9MVZGNTS547Z

The package's version comes from `Cargo.toml` (`X.Y.Z` becomes `X.Y.Z.0`), so a resubmission
needs no bump while `Cargo.toml` is ahead of the refused 0.10.6.0.

## Later (not done)

- Submitting from CI with the Microsoft Store Developer CLI (`msstore`, MIT) and a Partner Center
  API key, instead of uploading by hand.
- A Store badge and link in `README.md` once the listing is live.
