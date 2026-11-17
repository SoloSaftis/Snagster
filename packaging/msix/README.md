# Snagster's MSIX package (Windows)

`Snagster-windows-x64.msix` installs Snagster the way Windows prefers: it appears in Start and
in Settings > Apps, uninstalls cleanly, and updates in place. On company laptops it is also what
IT departments can hand out (Intune, Configuration Manager) and what endpoint security trusts
more readily than a loose, unsigned `.exe` in the Downloads folder.

- `AppxManifest.xml`: the package. `make-msix.ps1` fills in the version (from `Cargo.toml`) and
  the publisher.
- `make-msix.ps1`: builds the package with `makeappx` from the Windows SDK and signs it with
  `signtool`. The logos are drawn by `snagster --icon-png <file> <size>`.
- CI (`.github/workflows/build.yml`, Windows job) makes it on every push, installs it on the
  runner, and runs the button test and the MCP smoke test through the installed
  `snagster.exe` alias.

## What the package does differently from the portable zip

| | Portable zip | MSIX |
|---|---|---|
| Program folder | wherever you unpack it (writable) | `C:\Program Files\WindowsApps\…` (read-only, new folder per version) |
| Settings, crash.log | `%APPDATA%\Snagster` | the same folder (the manifest turns off AppData virtualisation, so both share it and Explorer sees it) |
| Updates | Snagster replaces its own `.exe` | Snagster downloads the new `.msix`, checks its SHA-256 against `SHA256SUMS`, and opens it in App Installer |
| Start with Windows | registry `Run` entry | the package's startup task `SnagsterStartup` (Settings > Apps > Startup) |
| AI apps (MCP) | the `.exe` path | `%LOCALAPPDATA%\Microsoft\WindowsApps\snagster.exe` (always the installed version) |
| `.snag` files | — | open in Snagster |

The code is in `src/packaged.rs` (package detection, startup task, App Installer) and
`src/update.rs`.

## Signing

Windows only installs an MSIX whose signature it trusts. Until Snagster has a code-signing
certificate, CI signs the package with a throwaway test certificate and keeps it out of the
release (the `windows-msix-test` artifact of each CI run has the package and the certificate).
To try that one by hand, an administrator imports `Snagster-test-signing.cer` into **Local
Computer > Trusted People**, then double-clicks the `.msix`.

Once there is a certificate:

1. Add the repository secret `MSIX_CERT_PFX` (the `.pfx`, base64: `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`)
   and `MSIX_CERT_PASSWORD`.
2. Add the repository variable `MSIX_PUBLISHER` with the certificate's subject, exactly as the
   certificate has it (for example `CN=Jane Doe, O=Jane Doe, L=Oslo, C=NO`).
3. The next tag's release then has seven files: the MSIX joins the five downloads and
   `SHA256SUMS`. Add its link to `README.md` and `.github/release-notes.md` at that point.

Changing the publisher later makes Windows treat the package as a different app (people would
have to uninstall the old one), so pick the certificate once.

Ways to get a certificate (none bought or set up yet):

- **Azure Artifact Signing** (formerly Trusted Signing), about USD 10 a month. Microsoft holds
  the key; signatures are trusted by Windows and build SmartScreen reputation. Identity
  validation is for organisations in the USA, Canada, the EU and the UK, and (as far as the
  documentation said in 2026) individuals only in the USA and Canada. Signing then runs in CI
  through Microsoft's GitHub action instead of the `.pfx` secret.
- **An OV code-signing certificate** from a certificate authority (for example Certum's
  open-source certificate for individuals). Since 2023 the key must live on a hardware token or
  in the authority's cloud signing service, so CI signs through that service rather than a
  `.pfx` file.
- **The Microsoft Store.** Registering as an individual developer is free; the Store signs the
  package itself, and many company laptops allow Store apps where they block other installers.
  The `unvirtualizedResources` capability needs a short justification in the submission.
