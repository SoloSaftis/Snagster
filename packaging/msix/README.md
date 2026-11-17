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

## Signing (Azure Artifact Signing)

Windows only installs an MSIX whose signature it trusts. Snagster is signed with **Azure
Artifact Signing** (formerly Trusted Signing), through Microsoft's GitHub action
[`azure/artifact-signing-action`](https://github.com/Azure/artifact-signing-action) (MIT). Once
it is set up, CI signs `Snagster.exe` (in the portable zip too, so SmartScreen and company
security software see a known publisher) and the MSIX, and the release gets the MSIX as a
seventh file.

Until then CI signs the package with a throwaway test certificate and keeps it out of the
release (the `windows-msix-test` artifact of each CI run has the package and the certificate).
To try that one by hand, an administrator imports `Snagster-test-signing.cer` into **Local
Computer > Trusted People**, then double-clicks the `.msix`.

### Who can get a certificate

Public-trust certificates go to **organisations** in the USA, Canada, the EU, the UK, Australia,
New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel, and to **individual
developers only in the USA and Canada** (Microsoft's quickstart, checked 2026-11-17). An
organisation needs a business identifier, a website and an email address on its own domain.
The certificate's name is the validated legal name; it can't be chosen. Validation takes 1 to 20
business days. It needs a paid Azure subscription (not a free or trial one); the Basic plan is
about USD 10 a month for 5,000 signatures.

### Setting it up (the person, in the Azure portal)

1. In a pay-as-you-go Azure subscription, register the resource provider
   `Microsoft.CodeSigning` (Subscriptions > Resource providers).
2. Create an **Artifact Signing account** (Basic) in a region near you, for example North
   Europe (endpoint `https://neu.codesigning.azure.net`).
3. Give yourself the **Artifact Signing Identity Verifier** role on the account, then create an
   **identity validation** (Public) and complete it.
4. Create a **certificate profile** of type **Public Trust** from that validation. Copy its
   **certificate subject** exactly.
5. For GitHub: in Microsoft Entra ID, register an app (for example `snagster-github-signing`),
   add a **federated credential** for GitHub Actions (organisation `SoloSaftis`, repository
   `Snagster`, entity type Tag, pattern `v*`, and a second one for branch `main` if you want
   signed test builds), and give the app the **Artifact Signing Certificate Profile Signer** role
   on the account. No password or secret is stored anywhere.
6. In the GitHub repository (Settings > Secrets and variables > Actions):
   - secrets `AZURE_CLIENT_ID` (the app's id), `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`;
   - variables `ARTIFACT_SIGNING_ENDPOINT` (from step 2), `ARTIFACT_SIGNING_ACCOUNT`,
     `ARTIFACT_SIGNING_PROFILE`, and `MSIX_PUBLISHER` (the subject from step 4, exactly).

The next tag then releases seven files. Add the MSIX link to `README.md` and
`.github/release-notes.md` at that point, and update the file count in the private notes.

Changing the publisher later makes Windows treat the package as a different app (people would
have to uninstall the old one), so it is chosen once.

### Later: the Microsoft Store

Registering as an individual developer is free and the Store signs the package itself; many
company laptops allow Store apps where they block other installers. The
`unvirtualizedResources` capability needs a short justification in the submission, and a
Store install must leave updates to the Store (the updater doesn't do that yet).
