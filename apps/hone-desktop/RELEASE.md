# Hone Desktop — Release Process

> Phase 5 slice C of the GUI roadmap. This doc covers the
> cross-platform packaging pipeline for `hone-desktop` (Tauri 2). The
> CLI binaries (`hone`, `honed`) ship through a separate workflow
> driven by `v*` tags — see `.github/workflows/release.yml`.

## TL;DR

```bash
# 1. bump apps/hone-desktop/src-tauri/tauri.conf.json `version` to 0.1.x
# 2. update CHANGELOG.md
# 3. tag and push
git tag desktop-v0.1.3
git push origin desktop-v0.1.3
```

The `desktop-release.yml` workflow takes over from there:

1. Builds the app on `macos-14` (arm64), `macos-13` (x86_64),
   `ubuntu-22.04`, and `windows-latest`.
2. Bundles each platform's installers (`.dmg`, `.app`, `.deb`,
   `.AppImage`, `.msi`).
3. Signs and notarizes when the appropriate secrets are present
   (see [Secrets](#secrets) below).
4. Publishes a **draft** GitHub release at `desktop-v<version>` with
   every bundle attached.

A release engineer reviews the draft, edits the body, then publishes.

## Installing the workflow

The CI workflow lives at **`apps/hone-desktop/ci/desktop-release.yml`**
in this repo. It is shipped under `apps/` rather than
`.github/workflows/` so that PRs touching it can be reviewed without
requiring the GitHub OAuth `workflow` scope on every contributor's
token. To activate it:

```bash
cp apps/hone-desktop/ci/desktop-release.yml .github/workflows/
git add .github/workflows/desktop-release.yml
git commit -m "ci: enable desktop release workflow"
git push   # requires a token with `workflow` scope
```

Once installed in `.github/workflows/`, GitHub picks it up automatically
on the next tag push.

## What CI runs

The workflow (`apps/hone-desktop/ci/desktop-release.yml` →
`.github/workflows/desktop-release.yml` once installed):

- Triggers on `desktop-v*` tags **or** manual `workflow_dispatch`.
- Uses [`tauri-apps/tauri-action`](https://github.com/tauri-apps/tauri-action)
  to wrap `cargo tauri build`. The action understands the `tauri.conf.json`
  bundle config and produces every target listed there.
- Uploads bundles as both:
  - GitHub release artefacts (the canonical distribution channel)
  - `actions/upload-artifact@v4` blobs (so a `workflow_dispatch` run
    that does NOT push to a release still leaves something
    downloadable for QA).

The matrix is intentionally fail-fast=false — a Linux builder hiccup
should not abort the macOS bundles.

## Secrets

The workflow reads these from repo secrets. Bundles are produced **even
when secrets are absent**, but they ship unsigned and are not suitable
for production. The release engineer is expected to mark such a build
as a prerelease and warn in the body.

| Secret | What for | Required for production? |
|---|---|---|
| `APPLE_ID` | Apple Developer account email | macOS notarization |
| `APPLE_PASSWORD` | App-specific password (NOT the account password) | macOS notarization |
| `APPLE_TEAM_ID` | Apple Developer team identifier | macOS notarization |
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` Developer ID Application cert | macOS signing |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12` | macOS signing |
| `APPLE_SIGNING_IDENTITY` | Common name from the cert (e.g. `Developer ID Application: Hone (TEAM)`) | macOS signing |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri updater signing key (base64) | auto-update |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the updater key | auto-update |

Windows EV signing is **not** wired yet — `windows.signCommand` in
`tauri.conf.json` is left blank, and the workflow ships unsigned `.msi`s
on Windows. Adding it requires:

1. An EV code-signing certificate on a hardware token / cloud HSM.
2. A Windows runner with the signing tooling installed (DigiCert
   KeyLocker, AzureSignTool, etc.).
3. A `WINDOWS_SIGN_COMMAND` secret pointing at the chosen signer.

When ready, set `bundle.windows.signCommand` in `tauri.conf.json` and
plumb the secret through `desktop-release.yml`.

## Generating signing keys (first time)

### Tauri updater key pair

```bash
# Run from anywhere; saves the pair to ~/.tauri/.
cargo tauri signer generate -w ~/.tauri/hone-updater.key
```

The command prints two artefacts:

- A **public key** — paste it into `tauri.conf.json` as
  `plugins.updater.pubkey`.
- A **private key** — base64-encode and store as the
  `TAURI_SIGNING_PRIVATE_KEY` repo secret. NEVER commit it.

### Apple Developer ID

1. Enroll in the Apple Developer program (one-time, $99/year).
2. Generate a Developer ID Application certificate via Xcode →
   Preferences → Accounts → Manage Certificates.
3. Export the cert + private key as a `.p12` from Keychain Access.
4. `base64 < cert.p12 | pbcopy` and paste into the `APPLE_CERTIFICATE`
   secret.
5. Generate an app-specific password at https://appleid.apple.com →
   Sign-In and Security → App-Specific Passwords. Paste into
   `APPLE_PASSWORD`.

## Auto-update

The updater plugin is **off by default**. To turn it on for a release:

1. Build with the `updater` cargo feature: `cargo tauri build --features updater`.
   The CI workflow does this automatically when `TAURI_SIGNING_PRIVATE_KEY`
   is present in the env.
2. Set `plugins.updater.active = true` in `tauri.conf.json`.
3. Stand up the release feed at the URL in
   `plugins.updater.endpoints[0]` (today: a placeholder pointing at
   `releases.hone.dev`).
4. After each release, generate the manifest with
   `cargo tauri signer sign` and upload it to the feed.

Until step 3 is done, leaving `active: false` is intentional — the
plugin is bundled but never polls.

## Updating bundle metadata

Edit `apps/hone-desktop/src-tauri/tauri.conf.json`:

- `version` — bump per semver (`MAJOR.MINOR.PATCH`).
- `bundle.publisher`, `bundle.copyright` — update if the legal entity
  changes.
- `bundle.macOS.minimumSystemVersion` — currently `11.0` (Big Sur).
  Bump only when an OS API we depend on requires it.
- `bundle.linux.deb.depends` — keep aligned with the actual runtime
  deps (currently webkit2gtk-4.1 + ayatana-appindicator3-1).

## Local sanity check (before tagging)

```bash
cd apps/hone-desktop/src-tauri
cargo tauri build               # produces target/release/bundle/
ls target/release/bundle/       # verify the artefacts you expect
```

If `cargo tauri build` succeeds locally on macOS, it almost always
succeeds in CI on the macOS runner. The most common CI-only failure
mode is missing Linux deps — those are pinned in `desktop-release.yml`.
