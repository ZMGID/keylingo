# System glass sidebar window design

## Boundaries

- `src-tauri/src/windows.rs` owns initial chat-window transparency and platform chrome.
- `src/chat/ChatWindowHost.tsx` owns the runtime native-effect lifecycle because Tauri exposes
  `setEffects`, `clearEffects`, focus events, and physical resize payloads through the window API.
- The existing settings model owns the preference. The Appearance group exposes one toggle.
- `src/index.css` owns active-material overlays and opaque fallback colors.

No chat, conversation, routing, agent, or sidebar data behavior changes.

## Settings Contract

Add a top-level boolean `translucentSidebar`, serialized by Rust as `translucent_sidebar` through the
existing camelCase bridge. Its default is `true`, so existing installations opt into the requested
appearance without migration code. Frontend normalization also defaults missing values to `true`.

`App` reads the setting in the same pass that applies theme state and passes it to
`ChatWindowHost`. Existing `onSettingsChange -> applyTheme` propagation updates it immediately.

## Effect State Machine

The host tracks the last physical size and uses a single predicate:

`enabled && width < 3840 || height < 2160`

expressed safely as `enabled && !(width >= 3840 && height >= 2160)`.

- macOS additionally requires focus. Eligible state applies Menu with
  `FollowsWindowActiveState`; ineligible state calls `clearEffects`.
- Windows ignores focus changes. Eligible state applies Mica. Any rejected application immediately
  calls `clearEffects` and enters opaque fallback.
- Linux and non-Tauri browser runs never apply a native effect.

The host exposes active/fallback state through one CSS class. Async calls use a monotonically
increasing request token so a late effect result cannot overwrite a newer focus, resize, or setting
decision.

Resize payloads are consumed directly as physical pixels. No scale-factor conversion occurs.

## Rendering

The macOS and Windows chat builders use transparent native backgrounds. The chat shell is transparent
only when a native effect is active. The sidebar paints a readable translucent overlay:

- light: `rgba(249, 249, 249, 0.72)`
- dark: `rgba(20, 20, 20, 0.66)`

The main pane and all full-page chat surfaces remain opaque (`#fff` / approximately `#171717`).
Fallback removes the active class, restoring an opaque shell/sidebar. No sidebar blur or
`backdrop-filter` is added.

Existing macOS Overlay title bar and traffic lights remain intact; Windows keeps its frameless
controls and DWM-owned frame.

## Failure And Compatibility

- Mica failure is non-fatal and always followed by `clearEffects`.
- Menu application failure uses the same opaque fallback.
- `clearEffects` failures are swallowed after the CSS fallback is established; window usability must
  not depend on compositor support.
- Linux window creation changes from transparent to opaque to match the unsupported contract.

## Verification

- Extract the eligibility/effect orchestration into a small testable hook/helper module.
- Unit-test inclusive 4K checks, macOS focus behavior, Windows focus persistence, setting disable,
  Linux fallback, and application failure.
- Component-test the Appearance toggle and perform the required mutation check.
- Run typecheck, lint, focused Vitest suites, production UI build, and Rust tests covering settings
  defaults/window helpers where practical.
