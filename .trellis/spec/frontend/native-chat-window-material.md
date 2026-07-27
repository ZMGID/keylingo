# Native Chat Window Material

## Scope

This contract applies only to the `chat` window and its conversation sidebar. Translator, Lens, and
settings window materials are unchanged.

## Platform Contract

- macOS uses `Effect.Menu` with `EffectState.FollowsWindowActiveState`.
- macOS clears the effect when the preference is off, the window is unfocused, or the physical
  window size is at least `3840 x 2160` on both axes.
- Windows uses `Effect.Mica`. Focus changes do not affect Mica.
- Windows clears the effect when the preference is off, the physical window size reaches the same
  two-axis threshold, or applying Mica fails.
- Linux does not call the native-effects API and uses an opaque window.
- Tauri resize payloads are already physical pixels. Never multiply them by `scaleFactor`.
- Do not substitute macOS Sidebar material or Windows Acrylic.

The persisted `translucentSidebar` setting defaults to `true`. Missing legacy values must normalize
to enabled on both the Rust and TypeScript sides.

## Rendering Contract

macOS and Windows use a transparent native/WebView background. While the native effect is active,
the chat shell is transparent and the sidebar adds only a readable color overlay. The neutral
colors are brightness baselines, not replacements for the configured theme hue.

For warm/cool themes, preserve the source surface's HSV hue and saturation and replace only its
value:

`scale = target_value / max(source_r, source_g, source_b)`

`derived_rgb = round(source_rgb * scale)`

| Theme | Light main | Light sidebar | Dark main | Dark sidebar |
| --- | --- | --- | --- | --- |
| Default | `#ffffff` | `#f9f9f9` | `#171717` | `#141414` |
| Warm | `#fffbf2` | `#f9f2e0` | `#171716` | `#141312` |
| Cool | `#f3f8ff` | `#e4eff9` | `#161617` | `#121314` |

The sidebar uses its derived RGB channels at `72%` opacity in light mode and `66%` in dark mode.
The main pane remains opaque. When the effect is inactive or fails, the shell and sidebar use the
same derived colors opaquely.

The sidebar must not use CSS blur or `backdrop-filter`. The platform compositor owns the blur.

## Test Contract

Tests must cover:

- inclusive two-axis physical-size cutoff;
- macOS focus loss and Menu state;
- Windows focus independence and Mica selection;
- disabled preference and unsupported Linux fallback;
- failed Mica application followed by `clearEffects`.
- each declared theme surface matching the HSV value calculation;
- opaque and native-material sidebar states sharing the same RGB source.
