# System glass sidebar window

## Goal

Apply platform-native macOS Menu and Windows Mica effects to the chat sidebar while preserving existing business behavior.

## Requirements

- Preserve the existing chat/sidebar business behavior and navigation.
- Add a user-facing "Translucent sidebar" appearance preference, enabled by default and persisted
  with the existing settings model.
- On macOS, the chat window must use Tauri's `Effect.Menu` with
  `EffectState.FollowsWindowActiveState`. It must never use the Sidebar material.
- On macOS, clear the effect and show an opaque fallback when the preference is disabled, the chat
  window loses focus, or both physical dimensions are at least `3840 x 2160`.
- On Windows 11, the chat window must use `Effect.Mica`, never Acrylic. Losing focus must not clear
  Mica.
- On Windows, clear the effect and show an opaque fallback when the preference is disabled, both
  physical dimensions are at least `3840 x 2160`, or Mica cannot be applied.
- Treat Tauri resize payloads as physical dimensions; do not multiply them by the scale factor.
- Linux must never enable a native effect and must always use the opaque fallback.
- The chat window background must be transparent on macOS and Windows while the sidebar uses native
  material.
- In light mode, use an approximately `rgba(249, 249, 249, 0.72)` sidebar overlay and an opaque white
  main pane.
- In dark mode, use an approximately `rgba(20, 20, 20, 0.66)` sidebar overlay and an opaque
  `#171717` main pane.
- Do not use CSS blur or `backdrop-filter` for the chat sidebar.
- Limit changes to the window material lifecycle, title-bar/window chrome, the appearance preference,
  and directly related styles/tests.
- Deliver small atomic commits without unrelated refactors or generated churn.

## Acceptance Criteria

- [x] macOS applies Menu material only while enabled, focused, and below the physical 4K threshold.
- [x] Windows applies Mica while enabled and below the physical 4K threshold, including while
      unfocused, and falls back cleanly when application fails.
- [x] Linux always renders the opaque fallback and performs no native-effect application.
- [x] Disabling the preference clears any active effect and immediately makes the sidebar opaque.
- [x] Resize handling uses the physical size payload directly and treats the threshold as inclusive
      on both axes.
- [x] Main content remains opaque in light and dark themes while only the sidebar reveals material.
- [x] Existing title-bar controls, sidebar collapse behavior, routes, and window sizing continue to
      work.
- [x] Focused unit/component tests cover the preference and effect lifecycle; TypeScript, ESLint,
      frontend tests, and relevant Rust tests pass.
- [x] Commits are independently reviewable and contain no unrelated changes.

## Notes

- "Main window" in this task refers to the primary chat window containing the sidebar, not the
  compact translator window.
