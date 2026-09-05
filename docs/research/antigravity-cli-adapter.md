# Antigravity CLI adapter

The external agent id is `antigravity`; the executable is Google's official `agy`.
This integration does not use the similarly named third-party IDE bridge.

## Protocol and lifecycle

- Launch with `--input-format stream-json --output-format stream-json`.
- Read `init.conversation_id` before registering a live session. Persist the actual
  native id and resume only with `--conversation <id>`. Reject a mismatched id.
- Send each prompt as a `user` event. One `result` finishes a turn, not the process.
- Changes to model, effort, permissions, attached directories or managed environment
  variables restart the process while retaining the native conversation binding.
- Cancel terminates the process tree. The next turn resumes the persisted id.
- A failed in-flight turn is not automatically replayed: agy has no request-id based
  acknowledgement/deduplication to prevent repeating tool side effects.
- Native session loss is surfaced as an error rather than silently starting over.

## Observations from agy 1.1.26 on Windows

- `init` arrives before any prompt is sent.
- Response chunks use `text_delta`; the final response repeats the text.
- Tool transitions include `ACTIVE`, `DONE` and `ERROR`. Tool errors must close cards.
- A denied tool can end in `SUCCESS` with an empty response and `denied_actions`.
  The adapter surfaces this as an actionable permission error.
- Per-step usage is local to that step; `result.usage` is cumulative across the
  conversation. Use per-step totals for the first resumed turn as well.
- Observed usage: input 5969, output 554, cache read 8132, native total 6523.
  Cache is separate from input, and thinking is already included in output.
  Kivio's context total therefore includes cache without adding thinking twice.
- `agy models` returns tab-separated model ids and labels.

## Supported boundary

Includes native installation/update discovery, model and effort selection,
permission modes, streaming text, tool cards, usage, multi-turn sessions and resume.
Permissions inherit the CLI configuration unless the user explicitly selects another
mode. Sign-in happens through an interactive `agy` terminal session.

The input stream supports text only; image attachments use the existing file-path
fallback. Runtime steering, native follow-up injection, approval RPCs, slash commands
and native-history import are not advertised by this adapter.

## Verification

Verified on Windows with agy 1.1.26: 697 external-agent unit/regression tests,
56 relevant frontend tests, TypeScript checking, and the live multi-turn/tool/cancel/
resume test all passed. Rust and final frontend checks ran in an isolated worktree
containing this adapter only, because unrelated plugin changes were being edited
concurrently in the original working directory.

Unit tests cover stream parsing, tool errors, soft denials, usage accounting, model
catalog parsing, launch flags, installer routing and reconnect/retry policy.

Desktop follow-up: the first in-app `agy models` probe hit its 30-second timeout;
a forced retry through the same Tauri command returned all 14 models. The probe now
allows 60 seconds and drops/kills its child on timeout. The picker displays the
backend error instead of only a generic fallback notice. This mitigates slow probes;
the CLI's intermittent delay has not been traced to an internal cause.

For debug builds, write a request with `prompt: ""`, `probeModels: true`,
`externalAgentId: "antigravity"`, and an optional `conversationId` to
`<app_data>/chat_probe/request.json`. The watcher runs the exact model-picker command
with cache bypass and writes `models-result.json`, without creating a chat turn.

The opt-in `live_antigravity_multiturn_tools_cancel_and_resume` Rust test uses an
isolated temporary workspace and a signed-in `agy`. It checks multi-turn recall,
file-tool events, cancellation, process replacement and native conversation recovery:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib antigravity
cargo test --manifest-path src-tauri/Cargo.toml --lib live_antigravity_multiturn_tools_cancel_and_resume -- --ignored --nocapture
```

References: [official headless protocol](https://antigravity.google/docs/cli/headless/),
[installation](https://antigravity.google/docs/cli/install/),
[official releases](https://github.com/google-antigravity/antigravity-cli/releases).
