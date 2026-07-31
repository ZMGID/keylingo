# Design

## Resource model

`src-tauri/icons/AppIcon.icon` is an Icon Composer package containing:

- `icon.json`: Default, Dark, and Tinted/Mono appearance annotations.
- `Assets/logo-mark.png`: the transparent Kivio glyph used as Liquid Glass artwork.

The top-level fill supplies light and dark system backgrounds. The artwork uses
appearance-specific fills while keeping one stable silhouette.

## Build integration

Add a cross-platform Node script invoked by Tauri's `beforeBundleCommand`.

- On macOS, locate `actool` through `xcrun`, compile `AppIcon.icon` into a
  generated resource directory, and verify `Assets.car` exists.
- On other platforms, exit successfully without requiring Xcode.
- Configure Tauri resources to place the generated `Assets.car` at the root of
  the macOS app's `Contents/Resources`.
- Merge the compiler-required icon keys into `src-tauri/Info.plist`.

The existing ICNS/ICO/PNG bundle configuration remains unchanged for backward
compatibility and non-macOS platforms.

## Verification

Validate the source package by exporting all six renditions with `ictool`.
Build a macOS app bundle, inspect `Contents/Resources`, inspect the compiled
plist, and verify the bundle signature after Tauri packaging.

## Risks

- `.icon` is a new package format. Validate against the installed Xcode 26.6
  tools rather than relying only on undocumented schema assumptions.
- Tauri doesn't natively model appearance variants. Precompiling the Apple
  resource before bundling avoids modifying an already signed app.
- New-system rendering is unavailable on older macOS releases, so `icon.icns`
  remains the fallback.
