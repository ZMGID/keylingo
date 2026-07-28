# Preserve theme hues in glass sidebar

## Goal

Restore chat theme hue support while matching the requested glass sidebar and main-pane luminance baselines through computed colors.

## Requirements

- Restore the existing default, warm, and cool theme-color behavior for the chat window.
- Treat the requested neutral colors as brightness baselines rather than universal fixed colors:
  light main `#ffffff`, light sidebar `rgb(249, 249, 249)`, dark main approximately `#171717`,
  and dark sidebar approximately `rgb(20, 20, 20)`.
- Derive warm and cool surface colors from the existing theme hue/saturation instead of replacing
  them with neutral gray.
- Use a documented, deterministic color calculation. For this fix, "brightness" means the HSV value
  channel: preserve the source theme color's hue and saturation, then replace its value with the
  neutral baseline's value.
- Use the derived sidebar color at `72%` opacity in light mode and `66%` opacity in dark mode while
  native material is active. Use the same derived RGB color opaquely for fallback.
- Keep the main content opaque.
- Do not change effect lifecycle, window configuration, business logic, or unrelated theme tokens.
- Deliver the correction as one atomic fix commit plus the required Trellis/spec update.

## Acceptance Criteria

- [x] Default light/dark colors remain the exact requested neutral baselines.
- [x] Warm and cool themes retain their hue in the main pane and sidebar.
- [x] Derived colors use the documented HSV value replacement and are recorded in CSS variables.
- [x] Native-material and opaque-fallback states use the same per-theme RGB source with only alpha
      changing.
- [x] No CSS blur or `backdrop-filter` is added to the sidebar.
- [x] Typecheck, lint, focused tests, UI build, and rendered default-theme check pass.

## Notes

- Proposed derived colors:

| Theme | Light main | Light sidebar | Dark main | Dark sidebar |
| --- | --- | --- | --- | --- |
| Default | `#ffffff` | `#f9f9f9` | `#171717` | `#141414` |
| Warm | `#fffbf2` | `#f9f2e0` | `#171716` | `#141312` |
| Cool | `#f3f8ff` | `#e4eff9` | `#161617` | `#121314` |
