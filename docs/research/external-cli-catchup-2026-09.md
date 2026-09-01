# External CLI adapter catch-up (2026-09-01)

**Question:** Which of the ten Kivio **external CLI agents** shipped protocol-relevant changes since the last adapter catch-up, and what should we patch?

**Last dedicated catch-up:** 2026-08-21 (`f4437c58` Claude 2.1.238, `56b44909` Codex 0.148, `e5851c20` dsh rc.8, `dff08acb` Grok `--no-leader`). Later related work: ACP terminal (`3b0ad43e` / `f62386ca`, Aug 25) and **附加目录** (`fa4ba69b`, Aug 28).

**Not in scope:** product code in this note. Implement from the ranked list below.

Kivio terms: **外部 CLI 代理** = installed CLI backing a conversation; **原生会话** = that CLI's own history; **会话绑定** = Kivio conversation ↔ native session id.

---

## Version matrix (2026-09-01)

| 外部 CLI 代理 | Kivio last pin | Latest | Gap | Adapter risk |
| --- | --- | --- | --- | --- |
| Claude Code | 2.1.238 host protocol | npm `2.1.252` | 14 patch versions | Medium — mostly TUI/enterprise; one stream-json host bug |
| Codex CLI | 0.148 schema (0.149 `permissionProfile` already avoided) | npm `0.152.0` | 0.149–0.152 | High — new item type + elicitation RPC |
| Cursor Agent | ACP v1 + `cursor-agent acp` | Aug 11 2026 CLI notes | ~3 weeks of CLI | High — blocking extension methods unanswered |
| OpenCode | ACP v1 `opencode acp` | npm `1.18.25` | 1.18.x bugfix train | Low — ACP v2 is experimental |
| Gemini CLI | `--experimental-acp` | npm `0.57.0` | flag renamed | Medium — deprecation, still works |
| Kimi Code | ACP + terminal host (0.37 glob research) | npm `0.39.1` | 0.38–0.39 | Low after their 0.39 ACP process-backend fix; re-test |
| Pi | RPC steer / follow_up | npm `0.84.4` | `clear_queue` | Medium — abort leaks queued messages |
| Hermes | `hermes acp --accept-hooks` | PyPI `0.21.0` | 0.20 → 0.21 | Low — desktop/bot, ACP logs redaction |
| Grok CLI | 1.0.4–1.0.5 + `--no-leader` | npm `1.0.13` | 1.0.6–1.0.13 | Low ACP; installer gap on Windows |
| DeepSeek Harness | 0.1.0-rc.8 SDK JSON-RPC | npm `0.1.1-rc.2` | rc.1 + rc.2 | Low — Files API images inside the CLI |

Sources: npm `latest` for the scoped packages; Claude [CHANGELOG](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md); Codex GitHub `rust-v0.149.0`…`rust-v0.152.0`; [Kimi changelog](https://www.kimi.com/code/docs/en/kimi-code-cli/release-notes/changelog.html); [Pi CHANGELOG](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/CHANGELOG.md); [Grok Build changelog](https://x.ai/build/changelog); [Hermes v0.21.0](https://github.com/NousResearch/hermes-agent/releases); [dsh-v0.1.1-rc.2](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.1-rc.2); [ACP updates](https://agentclientprotocol.com/updates); [Cursor CLI changelog](https://cursor.com/docs/cli/changelog); [Cursor ACP](https://cursor.com/docs/cli/acp).

---

## Ranked gaps (what to patch)

### P0 — protocol mismatch that can stall or drop a turn

1. **Cursor blocking ACP extensions.** Cursor documents `cursor/ask_question` and `cursor/create_plan` as **blocking** reverse RPCs: the agent waits for a JSON-RPC result. Kivio's ACP client answers `session/request_permission` and `terminal/*`, then `-32601 Method not found` for everything else with an `id` (`session/acp.rs` `handle_agent_to_client_request`). Plan approval / multi-choice questions therefore never reach `ask_user.rs` (codecs today: claude / dsh / pi / codex only). Notifications `cursor/update_todos`, `cursor/task`, `cursor/generate_image` can stay fire-and-forget (map to existing todo / subagent / image cards).

2. **Codex 0.152 item types + elicitation.** 0.148 catch-up rendered `dynamicToolCall` / `imageGeneration` / `sleep`. Unknown types still fall through `_ => {}` and the card vanishes. 0.152 enables **clock** tools (`#41210`) and `openai/elicitation` form requests (`#41447`). Today only `mcpServer/elicitation/request` is declined. A new request method without a handler is `-32601` and can hang the turn. Also: `thread/shellCommand` timeouts (`#41384`) — only matters if we ever call that RPC.

3. **Pi `clear_queue` (0.84.4).** Pi docs: clients must call `clear_queue` **before** `abort` to match Esc (return queued steer / follow-up text). Kivio's resident actor sends `{type:"abort"}` only (`pi_rpc.rs`). Queued 立刻引导 / follow-up stay in Pi's inbox and fire on the next prompt, or disappear from the Kivio composer.

### P1 — small, cheap, or user-visible installer/launch

4. **Gemini launch flag.** Stable docs and `--help` use `gemini --acp`; `--experimental-acp` is deprecated (still accepted). Kivio still launches `--experimental-acp` (`defs/acp.rs`). Switch primary argv to `--acp`, keep the old flag as a one-release fallback if we care about pre-rename binaries.

5. **Grok Windows install.** `installer.rs` has `script_windows: None` for grok. Official script exists: `irm https://x.ai/cli/install.ps1 | iex`. 1.0.13 also fixed opening `~/.grok` on Windows.

6. **Kimi 0.39 ACP process backend.** Their 0.39 notes fix Bash / Grep / Glob when the editor does **not** support terminal exec, plus session create/reopen. Kivio already advertises `terminal: true` (Aug 25 research: that *caused* the 0.37 glob failure). After 0.39, re-run the live glob probe; do not add `fs.readTextFile` just to paper over it (ACP host-adaptation note still holds).

### P2 — verify, don't invent work

7. **Claude 2.1.239–2.1.252.** Host-protocol relevant: 2.1.251 "client-injected assistant tool calls without a message id were merged" — Kivio's stream-json path sends **user** prompts, not synthetic `tool_use` assistants, so this is low unless we later inject. `--restricted` is a new sandbox mode we could expose as a capsule option; not a break. Todo env `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` still required (2.1.233). `--forward-subagent-text` still required.

8. **ACP spec (v1 stabilized after our pin).** Elicitation, `session/resume` (no history replay), `session/close`, `$/cancel_request`, boolean `configOptions`, `usage_update`. Kivio already sends `additionalDirectories` when the agent advertises it (Aug 28). Do **not** advertise `elicitation` until the ask-user card can complete form/url modes. `session/resume` is a cheaper reconnect than `session/load` for Kimi (they implement both); opencode import still needs `session/load` replay (ADR-0003).

9. **OpenCode 1.18.x.** ACP v2 exists behind `OPENCODE_EXPERIMENTAL_ACP_V2`. Stay on protocolVersion 1. Fallback models (`gpt-5`, `claude-sonnet-4-5`) are stale labels only; live probe fills the picker.

10. **dsh 0.1.1-rc.2.** Vision model + Files API image upload live **inside** the DeepSeek adapter. Kivio already lists `deepseek-v4-flash-vision-exp` and sends images through the SDK bridge. Plugin `latest` dist-tag skew (`dsh-*` plugins still pointing at older rc) is an upstream install footgun, not a wire change. Skip `0.1.2-alpha.*` until it is `latest`.

11. **Hermes 0.21 / Grok 1.0.6–1.0.13.** Almost all TUI / desktop. Grok ACP still `grok agent … --no-leader stdio`. Do not add a generic `agent` fallback binary: Grok's Windows installer also drops `agent.exe`, which would collide with Cursor's `agent` entrypoint.

12. **New 11th agent?** GitHub Copilot CLI now speaks ACP (`copilot --acp`). Product decision, not a catch-up of an existing def. Registry test still pins ten agents.

---

## Suggested implementation order

1. Gemini `--acp` argv + test update.
2. Cursor `cursor/ask_question` + `cursor/create_plan` → existing ask-user / plan cards; ignore or map notifications.
3. Pi: `clear_queue` then `abort`; restore returned texts into the Kivio queue if the user cancelled.
4. Codex: render `clock` (and any new item type we capture on a live 0.152 turn); handle `openai/elicitation` like `mcpServer/elicitation` until we have a real form UI (decline is better than `-32601`).
5. Grok Windows install spec.
6. Live smoke: Kimi 0.39 glob/grep with `terminal: true`; Claude 2.1.252 stream-json one turn; Codex 0.152 one turn with clock if the model emits it.

Do not bump `ACP_PROTOCOL_VERSION` to 2. OpenCode's v2 is still experimental; Kivio's handshake is v1 (`acp.rs` `ACP_PROTOCOL_VERSION = 1`).
