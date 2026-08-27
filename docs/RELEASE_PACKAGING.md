# Release Packaging

This document is the required release checklist for Kivio Desktop installers. Do not publish a new release only from memory; follow this file.

## Current Packaging Flow

Kivio Desktop is packaged by Tauri.

Local packaging (debug / inspect only — published installers come from GitHub Actions):

```bash
npm ci
npm run lint
npm run typecheck
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
```

`npm run build` runs:

1. `npm run build:swift`
   - Builds the macOS Swift sidecars.
   - On non-macOS platforms, creates stub binaries so Tauri `externalBin` validation passes.
2. `tauri build`
   - Runs `beforeBuildCommand` from `src-tauri/tauri.conf.json`, currently `npm run build:ui`.
   - Vite writes the production frontend to `dist/`.
   - Tauri packages `dist/`, configured `externalBin` files, configured `resources`, and platform icons into DMG / MSI / NSIS bundles.

GitHub release packaging (this is the official path — do not build installers locally):

1. Bump versions in `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and `src-tauri/tauri.conf.json`.
2. Write the bilingual GitHub body under `docs/releases/vX.Y.Z.md`. On the READMEs, **only** bump the version pointer in 功能 / Features (see [README format](#readme-format)). Do not restyle or replace `README.md` / `README.en.md`.
3. Commit and push `main`. Wait for `.github/workflows/ci.yml` on `main` to pass (lint / typecheck / frontend tests / Rust tests).
4. Create or move the release tag, for example:
   ```bash
   git tag -f vX.Y.Z
   git push origin main
   git push origin -f vX.Y.Z
   ```
   Pushing the `v*` tag is what starts packaging. To rebuild an existing tag after a workflow change:
   ```bash
   gh workflow run release.yml --repo ZMGID/kivio --ref main -f tag=vX.Y.Z -f ref=vX.Y.Z
   ```
5. `.github/workflows/release.yml` builds **both** installers on GitHub Actions and uploads them to the tag's release:
   - `macos-latest` (Apple Silicon / aarch64) with `--bundles dmg` → `Kivio.Desktop_X.Y.Z_aarch64.dmg`
   - `windows-latest` (x64) with `--bundles nsis` → `Kivio.Desktop_X.Y.Z_x64-setup.exe`
   - After the NSIS build, Windows also packs `scripts/package-windows-portable.ps1` → `Kivio.Desktop_X.Y.Z_x64-portable.zip` (unzip and run `Kivio Desktop.exe`; no Start Menu). In-app update still downloads the NSIS installer.
   - GitHub normalizes spaces in `productName` to dots in the asset file names.
   - The macOS DMG is **unsigned** (no signing secrets configured); first launch needs right-click → Open, or `xattr -cr "/Applications/Kivio Desktop.app"`.
6. Watch the workflow and inspect the release assets:
   ```bash
   gh run watch <RUN_ID> --repo ZMGID/kivio --exit-status
   gh release view vX.Y.Z --repo ZMGID/kivio --json url,assets
   ```
7. **Replace the CI-generated release body with hand-written bilingual notes.** The
   workflow publishes the release with a boilerplate body ("Automated macOS…");
   overwrite it to match the prior `v2.7.x` release format — title, a `## 下载 / Downloads`
   block (Windows NSIS + Windows portable zip + macOS DMG, plus the macOS "unsigned / first launch" note), a
   `## 新版本亮点 / What's New` bilingual bullet list (中文 + English inline per bullet,
   matching `docs/releases/vX.Y.Z.md`, not an inline README changelog), and a `完整变更 / Full changelog: …compare/vPREV...vX.Y.Z`
   link:
   ```bash
   gh release edit vX.Y.Z --repo ZMGID/kivio --notes-file docs/releases/vX.Y.Z.md
   ```

## Resources That Must Be Packaged

`src-tauri/tauri.conf.json` controls app resources. At minimum, document Skill releases must include:

```json
"resources": {
  "resources/skills": "skills"
}
```

The final installed app must contain:

- `skills/pdf/SKILL.md`
- `skills/docx/SKILL.md`
- `skills/xlsx/SKILL.md`
- `skills/obsidian-markdown/SKILL.md` (+ `references/`)
- `skills/obsidian-bases/SKILL.md` (+ `references/`)
- `skills/json-canvas/SKILL.md` (+ `references/`)
- `skills/obsidian-cli/SKILL.md`

> The four `obsidian-*` / `json-canvas` skills (adapted from kepano/obsidian-skills, MIT —
> see `resources/skills/NOTICE.md`) are gated at runtime on the Obsidian connector (a
> configured vault path), so they only surface to the model once the user sets an Obsidian vault.

## Release Verification

Before publishing or announcing installers, inspect the final artifact contents.

For macOS DMG:

```bash
mkdir -p /tmp/kivio-release-check
hdiutil attach -nobrowse -readonly -mountpoint /tmp/kivio-release-check \
  "src-tauri/target/release/bundle/dmg/Kivio Desktop_X.Y.Z_aarch64.dmg"
find "/tmp/kivio-release-check/Kivio Desktop.app/Contents/Resources" -maxdepth 5 -type f | sort
hdiutil detach /tmp/kivio-release-check
rmdir /tmp/kivio-release-check
```

For the local `.app` bundle before DMG:

```bash
find "src-tauri/target/release/bundle/macos/Kivio Desktop.app/Contents/Resources" -maxdepth 5 -type f | sort
```

For GitHub Releases:

```bash
gh release view vX.Y.Z --repo ZMGID/kivio --json url,assets
```

The release is not complete until the final installer contains loose `Contents/Resources/skills/pdf|docx|xlsx` Skill files.

## README format

GitHub's default landing page is **Chinese-first**, in the CC Switch README shape. Agents and humans updating the README for a release **must keep this layout**. Do not revert to the old short bilingual page (English-first header, inline “What's New” bullets, LINUX DO footer, no sponsor block, no star history).

### Files

| File | Role |
|---|---|
| `README.md` | Default. Chinese. |
| `README.en.md` | English. Same section order, same images, same links. |
| `README.zh-CN.md` | Stub that points at the two files above. Do not duplicate the body. |

Keep `README.md` and `README.en.md` in lockstep. A release bump that edits one must edit the other.

### Section order (do not reorder)

1. Centered header: `public/icon.png`, title, one-line tagline, badges (release / platform / Tauri / downloads / license), language switcher, download + 功能/文档 + QQ **1104450740**, QQ group image.
2. Two-paragraph pitch (tray / agent / bring-your-own-key). No LINUX DO or other 友链.
3. **❤️ 赞助 / Sponsor** — `<details open>`. Table: logo 150px in the left cell (`docs/sponsors/…`), sponsor-provided copy in the right cell. Copy is the sponsor's; do not append in-app setup steps (“设置 → 供应商 → 添加驱动…”). Contact line stays GitHub Issues + QQ.
4. 为什么用 Kivio / Why Kivio
5. 截图 / Screenshots (`docs/screenshots/`)
6. 功能 / Features — link [Releases](https://github.com/ZMGID/kivio/releases) **and** `docs/releases/vX.Y.Z.md`. **This version pointer is the only README line a release should change.** Do not paste the changelog into README.
7. 热键 / Hotkeys
8. 下载安装 / Download
9. 文档 / Documentation (table of repo docs)
10. 快速开始 / Quick start
11. 常见问题 / FAQ (`<details>`)
12. 开发 / Development (`<details>`)
13. 贡献 / Contributing
14. **Star History** at the end (chart, then License). Do not move it into the header as the only star widget.

### Release bump (example)

In `README.md` 功能:

```markdown
完整记录见 [Releases](https://github.com/ZMGID/kivio/releases) · 当前版本说明：[vX.Y.Z](docs/releases/vX.Y.Z.md)
```

In `README.en.md` Features:

```markdown
Full history: [Releases](https://github.com/ZMGID/kivio/releases) · current notes: [vX.Y.Z](docs/releases/vX.Y.Z.md)
```

Badges already resolve to `releases/latest`; do not hard-code the version in badge URLs.

A README-only bump is docs; `.github/workflows/ci.yml` skips `**.md` / `docs/**` / `LICENSE` and will not run the quality job. That is expected.

## Common Failure To Avoid

Do not treat "Skill files are bundled" as equivalent to "the host can parse those documents." `SKILL.md` only tells the model to use host `read`/`bash` tools. If Python or a PDF/Office CLI is missing, the agent should say so rather than inventing contents.

