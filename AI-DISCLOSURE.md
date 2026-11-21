# How Snagster is made: AI-generated material

Snagster is written with an AI coding assistant (Anthropic's Claude, through Claude Code). This
file records which parts of the project are AI-generated and to what extent, so anyone
packaging, reviewing or reusing Snagster can see it. It is kept current when that changes.

## What is AI-generated

| Part | Where | Extent |
|---|---|---|
| Program code | `src/`, `build.rs` | Essentially all of it |
| Tests | the `#[cfg(test)]` modules, `tests/`, the `--ui-test`, `--video-test` and `--stream-test` modes | Essentially all of it |
| Documentation | `README.md`, `CHANGELOG.md`, `.github/release-notes.md`, this file | Essentially all of it |
| Translations | `assets/i18n/` (85 language packs) | All of them (machine-translated by the AI, not yet reviewed by native speakers) |
| Build and release automation | `.github/workflows/` | Essentially all of it |
| Linux desktop entry and AppStream metainfo | `assets/linux/` | All of it |
| Snagster's own Flatpak packaging (the `.flatpak` on the releases page) | `packaging/flatpak/` | All of it. This is not a Flathub manifest |
| Windows installer and Microsoft Store edition | `packaging/msix/` (manifest, scripts, guides) | All of it |
| MCP bundles and the MCP Registry listing | `packaging/mcpb/`, `server.json` | All of it |
| Agent skill for AI coding agents | `skills/` | All of it |
| Microsoft Store listing texts | `packaging/msix/listing/` (86 languages) | All of them (written and machine-translated by the AI, not yet reviewed by native speakers) |
| Icon | drawn by code in `src/main.rs`; `assets/snagster-256.png` and `assets/linux/*.png` are renders of it | All of it |
| Screenshots | `assets/screenshots/` | Made by Snagster itself (`--screenshot`) from AI-written views |

## What a person does

The maintainer decides what is built and in what order, chooses between designs, tests
releases on real computers, and publishes them. Commits made with the assistant end with a
`Co-Authored-By: Claude` line, so `git log` shows which changes it made.

## Flathub

Flathub's [generative AI policy](https://docs.flathub.org/docs/for-app-authors/requirements#generative-ai-policy)
applies if Snagster is submitted there:

- The submission discloses the AI-generated material above (this file lists the parts and the
  extent).
- The Flathub manifest contains no AI-generated or AI-assisted content: it is written by hand
  by a person, and it is not kept in this repository. (`packaging/flatpak/` is a different,
  AI-generated manifest for Snagster's own download and is never used for Flathub.) AI tools
  in this project never write, edit or suggest lines for the Flathub manifest.
- AI tools never open or automate the submission pull request, and never write its commit
  messages, description, review comments or replies. No AI-agent review is requested.
- Reviewers decide on the disclosed material at their discretion; disclosure does not mean
  acceptance.
