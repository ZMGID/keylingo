# Preserve theme hues in glass sidebar design

## Calculation

For each existing warm/cool source surface, convert RGB to HSV, preserve hue and saturation, and set
the value channel to the requested neutral baseline:

`scale = target_value / max(source_r, source_g, source_b)`

`derived_rgb = round(source_rgb * scale)`

This produces a theme-tinted color whose brightest channel matches the requested baseline. It avoids
the white-collapse problem of forcing an OKLCH/HSL lightness of `100%`, which would remove all hue.

Sources:

- Main pane: `--theme-surface`
- Sidebar: `--theme-surface-muted`

The dark variants use the same source channel ratios scaled down to the `#171717` and `#141414`
value baselines.

## CSS Shape

Add two chat-scoped semantic variables:

- `--chat-main-surface`
- `--chat-sidebar-surface`

Define neutral defaults on `:root`, warm/cool light overrides on their existing
`data-theme-color` selectors, and warm/cool dark overrides on combined
`:root.dark[data-theme-color]` selectors.

All chat shell/main selectors consume `--chat-main-surface`. The sidebar consumes
`--chat-sidebar-surface` as RGB channels, using `rgb(var(...))` for fallback and
`rgb(var(...) / alpha)` for the native-effect overlay. No global `--theme-surface*` token changes.

## Verification

- Add a CSS contract test that parses the declared variables and verifies each derived color against
  the HSV calculation.
- Verify the rendered default theme and cover every light/dark theme declaration with the color
  contract test.
- Run the existing frontend validation suite and production build.
