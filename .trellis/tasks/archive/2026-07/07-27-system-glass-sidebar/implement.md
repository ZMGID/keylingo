# System glass sidebar window implementation

## Execution Plan

- [x] Add the persisted `translucentSidebar` setting with a default of `true`, frontend
      normalization, Appearance toggle, and focused tests.
- [x] Add a compact chat-window effect lifecycle module and wire it into `ChatWindowHost`.
- [x] Grant the chat window the Tauri set-effects capability and make platform window backgrounds
      match the macOS/Windows transparent and Linux opaque contracts.
- [x] Update chat shell, title-bar-adjacent surfaces, sidebar overlay, main-pane, dark-mode, and
      fallback CSS without adding CSS blur.
- [x] Add lifecycle tests for focus, physical resize threshold, platform behavior, and effect failure.
- [x] Perform mutation validation for new behavior-sensitive frontend tests and restore mutations.
- [x] Run `npm run typecheck`, `npm run lint`, focused `npm test`/Vitest, `npm run build:ui`, and
      relevant Cargo tests/checks.
- [x] Review the final diff for unrelated changes and redundant code.

## Atomic Commit Plan

1. `feat(settings): add translucent sidebar preference`
   Adds only the persisted preference, Appearance toggle, and its tests.
2. `feat(window): apply native sidebar materials`
   Adds window transparency/capability configuration, the effect lifecycle, related tests, and the
   minimal CSS/title-bar surface changes needed for active and fallback rendering.
3. `docs(spec): document native sidebar material contract`
   Updates only Trellis/project specification artifacts if the required finish-phase spec review
   identifies a durable contract to record.

Each code commit must pass its focused checks and be independently revertible.

## Rollback

- Revert commit 2 to return to the existing opaque chat window while leaving the inert preference.
- Revert commit 1 to remove the preference and restore the prior settings schema.
