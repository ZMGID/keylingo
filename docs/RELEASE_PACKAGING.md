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
2. Update README release notes and add the bilingual GitHub body under `docs/releases/vX.Y.Z.md`.
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
   block (both installers + the macOS "unsigned / first launch" note), a
   `## 新版本亮点 / What's New` bilingual bullet list (中文 + English inline per bullet,
   mirroring the README release notes), and a `完整变更 / Full changelog: …compare/vPREV...vX.Y.Z`
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
- Pyodide core runtime files
- `python_stdlib.zip`
- local Pyodide wheels for common document/data packages

> The four `obsidian-*` / `json-canvas` skills (adapted from kepano/obsidian-skills, MIT —
> see `resources/skills/NOTICE.md`) are plain markdown and do **not** require the Pyodide
> runtime. They are gated at runtime on the Obsidian connector (a configured vault path),
> so they only surface to the model once the user sets an Obsidian vault.

## Mandatory Python / Pyodide Offline Bundle

Bundled document Skills are not complete unless their Python execution runtime is bundled too.

When `pdf`, `docx`, and `xlsx` are shipped, the installer must also include an offline Pyodide package set for normal document analysis. Do not rely on the CDN path as the normal runtime path.

Required local Pyodide files:

- `pyodide.asm.js`
- `pyodide.asm.wasm`
- `pyodide-lock.json`
- `python_stdlib.zip`

Required local package wheels:

- `numpy`
- `pandas`
- `matplotlib`
- `pillow`
- `seaborn`
- `micropip`
- `openpyxl`
- `xlrd`
- `et_xmlfile`
- `pypdf`

Required local Python font assets:

- `NotoSansCJKsc-Regular.otf` for CJK text rendering in Python-generated matplotlib / Pillow images

Implementation requirement:

- Run `npm run prepare:pyodide` before the frontend build. It creates the reproducible Python sandbox runtime resources in `resources/python-sandbox/pyodide/`.
- Update the Vite Pyodide asset plugin in `vite.config.ts` so it emits the sandbox runtime core files, required local wheels, and required local font assets into `dist/pyodide/`; Tauri embeds that `frontendDist` asset tree into the application executable (it is not copied as loose files under `Contents/Resources`).
- Update `src/chat/pyodideRunner.ts` so `run_python` package loading prefers the bundled local `dist/pyodide/` package index and wheels.
- CDN package loading may remain as a fallback, but the app must be able to run normal `pdf` / `docx` / `xlsx` analysis without downloading those common packages at runtime.
- Do not package a host machine virtual environment or host `site-packages` as a substitute for Pyodide wheels. The runtime used by `run_python` is Pyodide in the WebView sandbox.

## Release Verification

Before publishing or announcing installers, inspect the final artifact contents.

For macOS DMG:

```bash
mkdir -p /tmp/kivio-release-check
hdiutil attach -nobrowse -readonly -mountpoint /tmp/kivio-release-check \
  "src-tauri/target/release/bundle/dmg/Kivio Desktop_X.Y.Z_aarch64.dmg"
find "/tmp/kivio-release-check/Kivio Desktop.app/Contents/Resources" -maxdepth 5 -type f | sort
strings -a "/tmp/kivio-release-check/Kivio Desktop.app/Contents/MacOS/kivio" | \
  rg 'pyodide/(pyodide\.asm\.(js|wasm)|python_stdlib\.zip|numpy-.*\.whl|pandas-.*\.whl|NotoSansCJKsc-Regular\.otf)'
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

The release is not complete until the final installer shows both:

- loose `Contents/Resources/skills/pdf|docx|xlsx` Skill files
- embedded `/pyodide/...` runtime, wheel, and font asset keys in the application executable

## Common Failure To Avoid

Do not treat "Skill files are bundled" as equivalent to "document analysis is bundled." `SKILL.md` only tells the model what to do. The Python/Pyodide runtime and common packages are the execution environment and must be packaged separately.
