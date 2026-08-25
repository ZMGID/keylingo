# ACP host adaptation for local coding CLIs

**Date:** 2026-08-25  
**Scope:** How a GUI **ACP Client** (Kivio) should adapt when driving many installed **external CLI agents** over Agent Client Protocol v1. Kimi Code 0.37 Glob/Grep failure is one sample, not the whole question.  
**Not in scope:** Product code changes. Claude Code is not a native ACP server; skip except where Zed’s adapter is mentioned.

Kivio terms (from `CONTEXT.md`): **external CLI agent** = installed CLI backing a conversation; **working directory** = directory the native session was created in; **native session** = the CLI’s own history, not Kivio’s.

---

## Problem statement

ACP lets an editor (Client) spawn a coding CLI (Agent) as a subprocess and speak JSON-RPC over stdio. The protocol is **bidirectional**: the Agent reports thoughts and tool cards, and it **may** call back into the Client for permissions, text file I/O, and process execution.

That callback surface is optional. Advertising a capability is not “extra RPC for free.” For several CLIs it **replaces** the local implementation the TUI uses. A host that advertises `terminal: true` can therefore make Bash work (host exec) while breaking Glob/Grep (agent still tries to spawn `fd`/`rg` through a host path that only accepts bash-shaped argv). A host that advertises `fs.readTextFile: true` can make text reads go through unsaved editor buffers — and, for some agents, **disable** a stronger local image/binary reader.

Kivio today (live chat sessions):

- Advertises `clientCapabilities.terminal: true` and implements `terminal/*`.
- Does **not** advertise or implement `fs.readTextFile` / `fs.writeTextFile`.
- Probe/import processes advertise `terminal: false` because they do not handle reverse RPC.

Kivio’s `acp_terminal.rs` comment currently claims Kimi/Cursor/OpenCode/Gemini route Bash **and** Glob/Grep through the host. That is true for **Kimi Code 0.37’s process backend**. It is **false** as a general ACP rule (OpenCode and Gemini keep glob/grep in-agent).

---

## Spec facts (ACP v1)

Primary sources:

- [Overview](https://agentclientprotocol.com/protocol/v1/overview)
- [Initialization](https://agentclientprotocol.com/protocol/v1/initialization)
- [File system](https://agentclientprotocol.com/protocol/v1/file-system)
- [Terminals](https://agentclientprotocol.com/protocol/v1/terminals)
- [Tool calls](https://agentclientprotocol.com/protocol/v1/tool-calls)
- [Schema](https://agentclientprotocol.com/protocol/v1/schema)
- [Architecture](https://agentclientprotocol.com/get-started/architecture)

### Who thinks vs who executes

The spec’s split is **not** “agent thinks, host always executes.”

- **Agent** owns the model loop, tool *selection*, and (by default) tool *execution*. It reports tool calls via `session/update` so the Client can render them. Quote: “While Agents handle the actual execution, they may leverage Client capabilities like permission requests or file system access.” ([tool-calls](https://agentclientprotocol.com/protocol/v1/tool-calls))
- **Client** owns UX, permission UI, and **optional** environment methods. Architecture: “ACP works when you're using a code editor to talk to a model you trust. … the code editor gives the agent access to local files and MCP servers.” ([architecture](https://agentclientprotocol.com/get-started/architecture))
- Reverse RPC is how the Agent asks the Client to read unsaved buffers, write files the editor can track, run a command in a Client-managed terminal, or ask the user for permission.

There is **no** ACP method for glob, grep, search, index, or directory listing. Schema `ClientCapabilities` lists `fs`, `terminal`, `auth`, `elicitation`, session config, `_meta` — not search. Tool kind `"search"` on `session/update` is a **display hint**, not an RPC. ([tool-calls](https://agentclientprotocol.com/protocol/v1/tool-calls) kinds: `read` / `edit` / `delete` / `move` / `search` / `execute` / `think` / `fetch` / `other`)

### Q1 — Host obligated vs optional; agent MUST if capability is false

**Obligated (baseline Client):**

- Call `initialize` before any session. Include latest supported protocol version and capabilities. ([initialization](https://agentclientprotocol.com/protocol/v1/initialization))
- Implement `session/request_permission` (the only baseline Client method). ([overview](https://agentclientprotocol.com/protocol/v1/overview))
- Treat omitted capabilities as **UNSUPPORTED**. “Clients and Agents MUST treat all capabilities omitted in the initialize request as UNSUPPORTED.” ([initialization](https://agentclientprotocol.com/protocol/v1/initialization))
- Paths in the protocol MUST be absolute; line numbers are 1-based. ([overview](https://agentclientprotocol.com/protocol/v1/overview))
- On cancel of the current prompt turn, permission requests MUST return `"cancelled"`. ([tool-calls](https://agentclientprotocol.com/protocol/v1/tool-calls))
- If `fs/write_text_file` is implemented: “The Client MUST create the file if it doesn't exist.” ([file-system](https://agentclientprotocol.com/protocol/v1/file-system))
- Terminal output truncation MUST happen at a character boundary. ([terminals](https://agentclientprotocol.com/protocol/v1/terminals))

**Optional (advertise only if implemented):**

| Capability | Methods | Schema default |
| --- | --- | --- |
| `fs.readTextFile` | `fs/read_text_file` | `false` |
| `fs.writeTextFile` | `fs/write_text_file` | `false` |
| `terminal` | `terminal/create`, `output`, `wait_for_exit`, `kill`, `release` | `false` |
| `auth.terminal` | Client can reproduce agent invocation in an interactive terminal for login | `false` |
| `elicitation` | `elicitation/create` (form / url modes) | omitted = none |
| session boolean config options | v1 `configOptions` `type: "boolean"` | omitted = unsupported |

Schema default for the whole object: `{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"auth":{"terminal":false}}`. ([schema ClientCapabilities](https://agentclientprotocol.com/protocol/v1/schema))

All capabilities in `initialize` are **OPTIONAL**. “Clients and Agents SHOULD support all possible combinations of their peer's capabilities.” Missing a capability MUST NOT crash the peer. ([initialization](https://agentclientprotocol.com/protocol/v1/initialization))

**Agent MUST if a capability is false / absent:**

- If `readTextFile` or `writeTextFile` is false or not present: “the Agent MUST NOT attempt to call the corresponding filesystem method.” ([file-system](https://agentclientprotocol.com/protocol/v1/file-system))
- If `terminal` is false or not present: “the Agent MUST NOT attempt to call any terminal methods.” ([terminals](https://agentclientprotocol.com/protocol/v1/terminals))

The spec does **not** say the Agent must then run those tools locally. Local fallback is **agent policy**. An agent that replaces its process layer with “host terminal or throw” and then throws when `terminal` is false is spec-legal on the MUST NOT call, and product-broken.

**Agent MUST on terminals it did create:** release with `terminal/release` when done; still release after `terminal/kill`. ([terminals](https://agentclientprotocol.com/protocol/v1/terminals))

**`terminal/create` shape:** `command` is the executable, `args` is argv. The spec does not require the Client to pass the line through a shell. A spec-conformant host `spawn(command, args)`s (execvp-shaped). Shell lines (`git status && …`) are the **agent’s** job to wrap.

**`{type:"terminal"}` tool content:** embeds a Client-created `terminalId` so the Client can show live output and keep showing it after release. ([tool-calls](https://agentclientprotocol.com/protocol/v1/tool-calls), [terminals](https://agentclientprotocol.com/protocol/v1/terminals)) This is display of a **Client-owned** process, not a search API.

**`session/request_permission`:** Agent MAY ask; Client MAY auto-allow/reject from settings. ([tool-calls](https://agentclientprotocol.com/protocol/v1/tool-calls))

### Q2 — Where glob/grep go; does the spec have search methods?

**Stay in-agent**, unless a particular agent chooses to implement them by spawning `rg`/`fd` through `terminal/create` (or wrapping them as `bash -c "rg …"`).

The spec has:

- No `fs/glob`, `fs/grep`, `fs/search`, `workspace/symbol`.
- `fs/read_text_file` / `fs/write_text_file` only (text; optional `line` + `limit` on read). Motivation: unsaved editor state + Client tracking of writes. ([file-system](https://agentclientprotocol.com/protocol/v1/file-system))
- `terminal/*` for **one command** in the Client environment. ([terminals](https://agentclientprotocol.com/protocol/v1/terminals))
- Tool kind `"search"` as UI category only.

So: glob/grep are **not** supposed to go through `fs/*`. They are **not** required to go through `terminal/*`. Hosts cannot “implement glob” in ACP v1.

### ACP v2 (official): fs/terminal move to MCP

v2 is published as **draft**; v1 remains the shipping protocol. ([llms.txt](https://agentclientprotocol.com/llms.txt), [v2 migration](https://agentclientprotocol.com/protocol/v2/migration))

Official decision ([RFD: v2 Client filesystem and terminal execution](https://agentclientprotocol.com/rfds/v2/client-filesystem-terminal-capabilities), [migration “Client file system and terminal execution removed”](https://agentclientprotocol.com/protocol/v2/migration)):

- Remove `clientCapabilities.fs`, top-level `clientCapabilities.terminal`, all `fs/*` and `terminal/*` methods, and Client-owned `{type:"terminal"}` semantics.
- Reason: “this surface was inconsistently implemented outside of a few IDEs, and agents already needed their own file and execution handling.”
- Replacement for **execution**: Client-provided **MCP servers** on `session/new` / `session/resume` (`mcpServers`).
- Replacement for **display**: Agent-owned `terminal_update` / `terminal_output_chunk` (not Client exec).
- `capabilities.auth.terminal` (login-in-a-terminal) is **kept**; it is not the v1 execution flag.

Kivio implication: v1 still needs a portable Client surface for today’s CLIs. Do not design Kivio’s long-term adaptation around inventing more v1 Client methods (glob, etc.). v2’s official answer for host-side tools is MCP, not more ACP RPCs.

---

## Host survey

| Host | Advertise | Implement `fs/*` | Implement `terminal/*` | Local vs proxy | Notes |
| --- | --- | --- | --- | --- | --- |
| **Zed** (first-party) | `fs.readTextFile` + `writeTextFile` true; `terminal` true; `auth.terminal` true; elicitation form+url; session boolean config; `_meta.terminal_output` + `terminal-auth`; Cursor-only `_meta.parameterizedModelPicker` | Yes (`handle_read_text_file` / `handle_write_text_file`) | Yes (create/output/wait/kill/release) | Host executes when agent calls back; many agents still exec locally | Same capability set for almost all agents; only `_meta` forks. Source: `zed-industries/zed` `crates/agent_servers/src/acp.rs` `client_capabilities_for_agent`. Live handshake Zed 0.214.7 in [zed#43819](https://github.com/zed-industries/zed/issues/43819). Docs: [zed.dev external agents](https://zed.dev/docs/ai/external-agents) |
| **JetBrains AI Assistant** (first-party) | Not in public source. Help docs: spawn subprocess, negotiate capabilities, optional pass-through of user MCP + IntelliJ MCP | Unknown (closed) | Unknown (closed) | Agent still a local subprocess; IDE may forward MCP | [jetbrains.com/help/ai-assistant/acp.html](https://www.jetbrains.com/help/ai-assistant/acp.html) (22 Jul 2026). Kotlin SDK **example** client advertises `fs` true: `JetBrains/acp-kotlin-sdk`. Do not treat the example as the IDE’s production payload |
| **VS Code `formulahendry/vscode-acp`** (community, listed on official clients page) | `{ fs: { readTextFile: true, writeTextFile: true }, terminal: true }` | Yes — `FileSystemHandler` reads **unsaved** `TextDocument` when open | Yes — `TerminalHandler` | Host FS + host terminal when agent calls | `src/core/ConnectionManager.ts` `connection.initialize(...)`. README: File System Integration + Terminal Execution. **Not** Microsoft first-party |
| **Cursor CLI as agent** | Cursor’s **own** public minimal client example advertises `{ fs: { readTextFile: false, writeTextFile: false }, terminal: false }` | N/A (agent side) | N/A | Agent-local tools unless a host advertises capabilities | [cursor.com/docs/cli/acp](https://cursor.com/docs/cli/acp). Cursor agent source is not public. Hosts (Zed) still advertise full fs+terminal **to** Cursor |
| **acpx** (`openclaw/acpx`) | Default **both true**; `--no-fs` / `--no-terminal` opt out | Yes | Yes | Host methods honor `--cwd` boundary | `src/acp/client.ts` `resolveClientCapabilities`; `docs/CLI.md`; [acpx.sh/permissions](https://acpx.sh/permissions.html). Integration test: default initialize has `terminal: true` and `fs: { readTextFile: true, writeTextFile: true }` |
| **Paseo** (`getpaseo/paseo`) | Default **all false**; providers **opt in** | Implemented when opted in | Implemented when opted in | Default: keep I/O in the agent environment (containers / remote) | `packages/server/src/server/agent/providers/acp-agent.ts` `BASE_ACP_CLIENT_CAPABILITIES` = `{ fs: { readTextFile: false, writeTextFile: false }, terminal: false }`. Docs: [custom-providers.md](https://github.com/getpaseo/paseo/blob/main/docs/custom-providers.md). PR [#2024](https://github.com/getpaseo/paseo/pull/2024) flipped the default **away** from Zed-style full advertise |
| **Python SDK example** (`agentclientprotocol/python-sdk` `examples/gemini.py`) | `fs` both true + `terminal` true | Example client reads/writes local files | Example implements terminal | Demo host | Reference client, not a product |
| **Kivio** (this repo, 2026-08-25) | Live: `{ terminal: true }` only. Probe/import: `{ terminal: false }` | No | Yes (`acp_terminal.rs`, execvp `Command::new(command).args(args)`, session cwd, `TERM=dumb` / `NO_COLOR=1`) | Host exec for `terminal/*` only | `src-tauri/src/external_agents/session/acp.rs` `acp_initialize_params` |

**Two host philosophies:**

1. **Zed / acpx / vscode-acp:** advertise the full v1 Client execution surface; agents that want local I/O simply do not call it.
2. **Paseo (after #2024):** advertise nothing by default so agents that **replace** local spawn when they see `terminal: true` keep their TUI backends. Opt in per provider.

Kivio is currently a third shape: **terminal-only**. That is legal (schema defaults fs to false) and happens to be the combination that makes Kimi 0.37 Bash work and Glob/Grep fail.

---

## Agent survey

| Agent | If host advertises `fs` | If host advertises `terminal` | Glob / Grep | If host advertises neither |
| --- | --- | --- | --- | --- |
| **Kimi Code 0.37 (TS, `kimi acp`)** | Text read/write proxied when flags true; else local `HostFileSystem`. Binary/bytes stay local | **All** `process.spawn` go through `AcpProcessService` → `terminal/create`. Gate: must look like bash `-c` + `NO_COLOR=1` + `TERM=dumb`. Else throw **before RPC** | Spawn `fd`/`rg` fails the bash-only gate. TUI still local-spawns, so TUI works | `ACP terminal capability is unavailable` — Bash **and** glob/grep die |
| **kimi-cli (Python, older)** | `ACPKaos.readtext` / `writetext` if flags; else local. `glob` / `iterdir` / `stat` / `exec` **always local fallback** | `replace_tools`: **Shell tool only** swapped for ACP `Terminal` (`create_terminal`). `ACPKaos.exec` still `_fallback.exec` | Stay on local kaos glob / grep_local — not routed through terminal | Shell stays local; fs stays local |
| **Gemini CLI (`gemini --acp`)** | If `clientCapabilities.fs` present, install `AcpFileSystemService`. Per-flag fallback to native FS; paths outside session root or under `~/.gemini` always native | Client-hosted Shell via `terminal/*` was **not** the default path in the sources reviewed; shell stays agent-side | `packages/core/src/tools/glob.ts` uses node `glob()`. Grep: git grep / system grep / JS. Not `terminal/*` | Native FS + native glob/grep |
| **OpenCode (`opencode acp`)** | Does not store/require fs for builtin read/write. Builtin tools use local FS. Some paths may `writeTextFile` **after** a local edit to refresh editor buffers | `BashTool` uses local `ChildProcess`. `terminal/create` **not called** (OpenVibeCoding memo of `packages/opencode/src/tool/bash.ts`) | Local grep/glob tools; ACP maps them to tool kind `search` for **display** (`packages/opencode/src/acp/agent.ts` `toToolKind`) | Same as TUI — ACP is a control/display plane |
| **Cursor (`agent acp`)** | Public docs do not document routing. Zed advertises fs+terminal to it anyway | Same | Unknown (closed source). Do not assume Kimi-style spawn replacement | Official example client uses fs false, terminal false — implies Cursor **can** run without host exec |
| **Hermes (`hermes acp`)** | Internals: file/terminal **Hermes tools** bound to editor **cwd**, not documented as calling `fs/*`. Issue #569 listed editor fs/terminal as an **option** | `terminal` / `process` / `execute_code` are Hermes tools (`acp_adapter/tools.py` kind map). Display as `execute` | `search_files` is a Hermes tool (`kind: search`), in-agent | Tools still run in Hermes process at session cwd |
| **pi / oh-my-pi** | Bridge sets `readTextFile` / `writeTextFile` from client caps; file tools may proxy | `BashTool` uses `createTerminal` when `terminal: true`. **Must** wrap the shell line: `wrapShellLineForClientTerminal` → `{ command: shell, args: [...shellArgs, line] }` | Not through terminal in the bash wrap PR; file search stays agent tools | Bash local; no `terminal/*` calls |
| **Grok CLI** (not in Kivio ACP family; cited as capability-hazard) | `readTextFile: true` routes `read_file` through text-only client method → `Cannot read binary file` on PNG. `false` → local image read works | `terminal/create` fires **even in Plan mode** on tested builds | n/a | Host can withhold `readTextFile` on purpose |

### Kimi Code 0.37 (the sample)

**Official docs are stale relative to 0.37.** [kimi acp reference](https://www.kimi.com/code/docs/en/kimi-code-cli/reference/kimi-acp.html) still says:

> `terminal/create` · `output` · `release` · `kill` · `wait_for_exit` — **No**. Terminal reverse-RPC not connected; shell commands use local execution.

That matches an older adapter, not 0.37’s process service.

**Local 0.37 binary** (Kivio probe + embedded `packages/acp-server/src/acp-terminal/acpTerminalRunner.ts`, 2026-08-25):

```js
async spawn(command, args = [], options) {
  if (!this.connection.terminalEnabled)
    throw new Error("ACP terminal capability is unavailable");
  if (!isBashToolInvocation(args, options))
    throw new Error("ACP runtime only supports interactive Bash tool processes");
  return this.connection.get().createTerminal({ command, args, ... });
}
function isBashToolInvocation(args, options) {
  return args.length === 2 && args[0] === "-c"
    && options?.env?.["NO_COLOR"] === "1"
    && options?.env?.["TERM"] === "dumb";
}
```

Independent corroboration that 0.37.2 expects host terminals for Bash **and** Glob/Grep: [multica-ai/multica#7323](https://github.com/multica-ai/multica/pull/7323) (“Advertise ACP terminal capability for the Kimi runtime so Kimi 0.37.2 can route Bash/Glob/Grep through the client”). That host implemented `terminal/*` so those tools could run; it does not add glob RPCs.

Kivio `AcpTerminalHost` already sets `TERM=dumb` and `NO_COLOR=1` on spawn (`acp_terminal.rs`). That satisfies Kimi’s **bash** env check on the host side. It cannot help Glob/Grep: those never reach `terminal/create`.

**kimi-cli Python** (still useful as contrast): `MoonshotAI/kimi-cli` `src/kimi_cli/acp/tools.py` `replace_tools` only replaces `Shell`. `src/kimi_cli/acp/kaos.py` `ACPKaos.glob` / `exec` delegate to `local_kaos`. [AGENTS.md](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/acp/AGENTS.md): “If the client advertises `terminal` capability, the Shell tool is replaced by an ACP-backed Terminal tool.” Not the whole process table.

Kimi `create_terminal` historically passed a **raw command string** with no `args` ([kimi-cli#1517](https://github.com/MoonshotAI/kimi-cli/issues/1517)) — broken on Windows execvp hosts. Same class of bug as oh-my-pi before the wrap.

### Gemini

- Docs: [google-gemini/gemini-cli `docs/cli/acp-mode.md`](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md) — “when the agent needs to read or write files, it does so through the ACP client” (security: only files the client allowed). Also: Client may expose **MCP** tools at initialize / session.
- `packages/cli/src/acp/acpClient.ts`: on `newSession` / `loadSession`, if `this.clientCapabilities?.fs`, wrap FS with `AcpFileSystemService`.
- `packages/cli/src/acp/acpFileSystemService.ts`: `readTextFile` / `writeTextFile` call the Client only when the corresponding flag is true; else fallback. Always fallback outside session root or under `~/.gemini`.

### OpenCode

- Docs: [opencode.ai/docs/acp](https://opencode.ai/docs/acp/) — “OpenCode works the same via ACP as it does in the terminal. All features are supported” including “Built-in tools (file operations, terminal commands, etc.).”
- `packages/opencode/src/acp/agent.ts` `initialize`: reads `_meta["terminal-auth"]` for login; does **not** branch builtin tools on `clientCapabilities.terminal`.
- Source memo (TencentCloudBase/OpenVibeCoding `docs/opencode-acp-integration-memo.md`): BashTool = local child process; no ACP terminal methods.

### Cursor

- [cursor.com/docs/cli/acp](https://cursor.com/docs/cli/acp): `agent acp`, stdio JSON-RPC, `session/request_permission`, Cursor extension methods (`cursor/ask_question`, `cursor/create_plan`, …). Minimal client initialize uses **fs false, terminal false**.
- Zed still sends full fs+terminal + `_meta.parameterizedModelPicker` for Cursor ([zed#57571](https://github.com/zed-industries/zed/issues/57571)).

### Hermes

- [hermes-agent.nousresearch.com ACP internals](https://hermes-agent.nousresearch.com/docs/developer-guide/acp-internals): session cwd bound to Hermes task; file and terminal **tools** run relative to editor workspace.
- `NousResearch/hermes-agent` `acp_adapter/tools.py`: `search_files` → kind `search`; `terminal`/`process` → `execute`. Rendering helpers, not Client `terminal/create`.
- Issue [#569](https://github.com/NousResearch/hermes-agent/issues/569) design listed optional editor `fs/*` and `terminal/create`. Treat current shipping behavior as **in-agent tools + cwd bind**, not Kimi-style spawn replacement, unless a later Hermes version is measured.

### pi / oh-my-pi

- [can1357/oh-my-pi#4335](https://github.com/can1357/oh-my-pi/pull/4335): ACP `command` is executable; wrapping is **agent-side**. `packages/coding-agent/src/tools/bash.ts` `wrapShellLineForClientTerminal`.
- ACP client bridge (`acp-client-bridge.ts`): `terminal: clientCapabilities?.terminal === true`; file flags independent.

---

## Capability matrix: what happens

| Host advertise | Kimi Code 0.37 | Gemini | OpenCode | Hermes (cwd-bound tools) | pi bash | Grok (if ever) |
| --- | --- | --- | --- | --- | --- | --- |
| **fs + terminal** (Zed) | Bash via host. Glob/Grep still bash-gated unless Kimi wraps `rg` as `bash -c`. Text r/w via host | Text r/w via host (in root). Glob/grep local. Shell mostly local | Unchanged local I/O; maybe buffer sync writes | Tools in Hermes at cwd; may ignore fs | Bash via host (wrapped argv). Files may proxy | Image `read_file` can break if `readTextFile` true |
| **terminal only** (Kivio today) | Bash works. Glob/Grep throw pre-RPC. Text r/w **local** (good) | Native FS. Glob/grep local | Unchanged | Unchanged | Bash via host | `terminal/create` still fires; Plan can mutate via host exec |
| **fs only** | Bash **and** glob/grep throw (`terminal` unavailable). Text r/w via host | Text r/w via host. Glob/grep local | Unchanged | Unchanged | Bash local | Image read may break; no host shell |
| **neither** (Paseo default) | Everything that goes through `AcpProcessService` throws | Native FS + native tools | Unchanged (best match) | Unchanged | Local bash | Local tools; no host exec |

---

## Q4 — Can a host “fix” Kimi-style bash-only spawn by implementing more methods?

**No.** Hypothesis confirmed.

- Glob/Grep never become ACP methods. Implementing `fs/*` does not give Kimi a search RPC.
- Kimi 0.37 rejects non-bash spawn **before** `terminal/create`. Extra terminal methods (`output`, PTY, stdin) are irrelevant.
- Implementing `fs.readTextFile` might proxy **Read**, not Glob.
- The only host-side mitigations:
  1. Keep `terminal: true` so Bash works (already done).
  2. Do **not** turn off `terminal` hoping glob returns to TUI spawn — 0.37 then fails closed on **all** process tools.
  3. Surface the agent error as agent policy, not a Kivio bug.
  4. Upstream: Kimi should keep `fd`/`rg` on local spawn (kimi-cli / OpenCode / Gemini pattern), or wrap them as `bash -c "rg …"` so they pass `isBashToolInvocation`.

A host that **shell-interprets** `command` (contrary to spec) still never sees Glob’s `fd` argv.

---

## What the host can vs cannot fix

| Can fix (host) | Cannot fix (agent / spec) |
| --- | --- |
| Implement advertised methods (`terminal/*`, later `fs/*`) | Invent glob/grep RPC other CLIs will not call |
| Advertise only capabilities you implement | Force Kimi to local-spawn `rg` while `terminal: true` |
| Probe/import: `terminal: false` so short-lived processes do not owe reverse RPC | Make `terminal: false` restore Kimi TUI process backend (0.37 throws instead) |
| execvp `command`+`args`; cwd absolute; env; output byte cap; kill process group | Parse arbitrary shell lines because one agent forgot to wrap (pi bug was agent-side) |
| `session/request_permission` UX; do not yolo `terminal/create` in Plan/read-only | Hermes denied-edit → later `terminal` bypass ([hermes#31682](https://github.com/NousResearch/hermes-agent/issues/31682)) — agent must re-prompt |
| Per-agent **withhold** a capability when advertising it disables a better local path (Grok images) | MCP “extra glob tool” stopping the model from calling the broken builtin Glob |
| Document per-CLI quirks; classify tool errors as host-RPC vs agent-policy | v1 spec adding search methods (v2 officially drops this Client surface) |

---

## Q5 — Portable adaptation layer for a multi-CLI host (Kivio)

Align with **Zed’s method set**, **Paseo’s “advertising replaces backends” lesson**, and **acpx’s cwd-bounded host exec**.

### Capabilities (live chat sessions)

1. **Keep `terminal: true`** and the five `terminal/*` methods. Required for Kimi 0.37 Bash; harmless for OpenCode/Gemini if they never call it.
2. **Add `fs.readTextFile` + `fs.writeTextFile`** as the next portable step — not to fix Kimi Glob, but to match Gemini proxy FS, kimi-cli/kimi-code text bridge, vscode-acp unsaved-buffer reads, and OpenCode’s optional buffer sync. Implement:
   - absolute paths;
   - `line`/`limit` on read (1-based);
   - create-on-write;
   - prefer working-directory confinement as **host policy** (spec does not require it; Kivio already notes this in `AcpTerminalHost::set_extra_roots`).
3. **Do not advertise fs/terminal on probe/import** processes.
4. **Later / as UI exists:** `auth.terminal` + `_meta.terminal-auth` (Kimi/OpenCode/Zed login), elicitation, session boolean config. Cursor: `_meta.parameterizedModelPicker` if Kivio grows a parameterized model picker ([zed#57571](https://github.com/zed-industries/zed/issues/57571)).
5. **Per-agent withhold list**, not a per-agent fake glob:
   - Default: Zed-like full surface.
   - Exception candidates: agents proven to **degrade** when a flag is true (Grok `readTextFile` vs images). Measure Cursor/Hermes before assuming.

### Methods

- Baseline: `session/request_permission` (already).
- `terminal/*`: keep execvp; do **not** wrap in `cmd /c` / `sh -c` on the host (pi’s lesson). Kivio already execvp-spawns.
- `fs/*`: text only. Do not pretend binary/image reads exist (Grok lesson).
- Embed `{type:"terminal", terminalId}` in the chat UI when present (spec SHOULD keep showing output after release).

### Permission + cwd / sandbox

- `terminal: true` means Kivio **is** the exec plane. Grok Plan still sent `terminal/create` to the client ([phuryn/grok-build-vscode `docs/internal/ACP-feedback.md`](https://github.com/phuryn/grok-build-vscode/blob/main/docs/internal/ACP-feedback.md)). Gate dangerous exec on `session/request_permission` and/or host policy — do not assume the agent’s Plan mode will withhold reverse RPC.
- Honor `cwd` on `terminal/create` (must be absolute). Default to the conversation **working directory**.
- acpx: fs/terminal honor cwd boundaries. Kivio currently does **not** confine terminal cwd to extra writable roots (comment in `acp_terminal.rs`) because agents pass temp dirs. Keep that honest; document it; don’t silently expand glob.

### MCP

Pass through `mcpServers` on `session/new` (already in Kivio’s ACP path for several agents). v2’s official “host tools” story is MCP. Do not build a parallel ACP glob API.

---

## Q6 — Per-agent quirks to document, not paper over

| Agent | Quirk |
| --- | --- |
| **Kimi Code 0.37** | `terminal: true` replaces **all** spawn; only bash `-c` + dumb/NO_COLOR. Glob/Grep red-card. Official acp matrix still says terminal RPC is disconnected. `session/load` binds but historically did not replay history ([ADR-0003](../adr/0003-acp-agents-import-via-protocol.md)) |
| **kimi-cli (Python)** | Only Shell → ACP terminal; glob stays local. Do not assume TS Kimi Code equals kimi-cli |
| **OpenCode** | ACP ≈ display + permissions. Builtin grep/glob/bash stay in the CLI process. Advertising terminal does nothing for those tools |
| **Gemini** | FS proxy when `fs` advertised; glob/grep stay in-core. `~/.gemini` and out-of-root paths never proxy |
| **Cursor** | Closed agent; extension methods `cursor/*`; parameterized model picker `_meta`; official sample client uses no fs/terminal |
| **Hermes** | Tools run in Hermes at session cwd; `search_files` is not ACP search RPC; edit deny may not cover later `terminal`/`execute_code` |
| **pi** | Must wrap bash for execvp hosts; raw `command` string → ENOENT |
| **Grok** (if ACP) | Withholding `readTextFile` can be the correct host choice |
| **All** | `{type:"terminal"}` is Client-owned live output, not a glob result. Agent-local tools still show as ordinary `tool_call` cards |

Do **not** special-case Kivio’s initialize payload per CLI except where advertising a flag is known to **disable** a needed local path.

---

## Q7 — Recommended Kivio next steps

1. **Keep `terminal: true` on live sessions.** Turning it off does not restore Kimi TUI glob; it kills Bash too.
2. **Implement `fs/read_text_file` + `fs/write_text_file` and advertise them** — portable IDE-class Client surface (Zed/acpx/vscode-acp). Expected wins: Gemini proxy FS, Kimi text bridge, unsaved-buffer semantics if Kivio ever has them. **Not** a Glob fix.
3. **Do not emulate glob/grep** in ACP or by rewriting `terminal/create` into a search engine.
4. **Fix the comment** in `acp_terminal.rs` that says all ACP CLIs route Glob/Grep through the host — that is Kimi 0.37 process policy.
5. **UI:** distinguish (a) host RPC errors, (b) permission deny, (c) **agent-policy** strings (`ACP runtime only supports interactive Bash tool processes`, `ACP terminal capability is unavailable`). Do not brand (c) as a Kivio outage. Kimi can often finish via Bash `find`/`grep` after the red cards.
6. **Upstream Kimi:** keep non-shell tools on local spawn (kimi-cli pattern) **or** wrap `rg`/`fd` as bash `-c`. Point at the stale official “terminal not connected” matrix.
7. **Do not rush ACP v2** for this: v2 *removes* Client fs/terminal. Stay on v1; when v2 stabilizes, host tools move to MCP.
8. **Optional later:** elicitation, `auth.terminal`, Zed `_meta`, Cursor parameterized picker, Plan-mode host policy on `terminal/create`.
9. **Security:** treat every `terminal/create` as real exec; permission + working-directory policy; no implicit shell.

---

## Answers (checklist)

1. **Obligated vs optional:** Host MUST initialize, treat omitted caps as unsupported, implement permission; fs/terminal/auth/elicitation are optional with schema defaults false. Agent MUST NOT call methods for false/omitted caps; MUST release terminals it created. Local fallback is not specified.
2. **Glob/grep:** In-agent (or agent-chosen `terminal/create` / `bash -c`). Spec has **no** search methods.
3. **Matrix:** Zed-style both-true is the common IDE host; Paseo-style neither is the safe default when agents replace backends; Kivio terminal-only is the Kimi-Bash-works / Kimi-Glob-fails cell. See tables above.
4. **Fix Kimi glob by more host methods?** No, unless Kimi uses `fs` (it doesn’t for glob) or wraps `rg` as bash `-c`.
5. **Portable layer:** Advertise+implement v1 `terminal/*` + `fs/*` + permission + cwd/sandbox; per-agent withhold only when a flag disables a better local path; MCP for extra tools; no custom glob RPC.
6. **Quirks:** Document Kimi spawn gate, OpenCode local I/O, Gemini FS proxy vs local search, pi execvp wrap, Cursor closed+extensions, Hermes in-agent search, Grok readTextFile/images, stale Kimi docs.
7. **Next:** Keep terminal; add fs; don’t emulate glob; show agent-policy errors honestly; file upstream with Kimi.

---

## Sources

### Official ACP

- https://agentclientprotocol.com/protocol/v1/overview
- https://agentclientprotocol.com/protocol/v1/initialization
- https://agentclientprotocol.com/protocol/v1/file-system
- https://agentclientprotocol.com/protocol/v1/terminals
- https://agentclientprotocol.com/protocol/v1/tool-calls
- https://agentclientprotocol.com/protocol/v1/schema (`ClientCapabilities` defaults)
- https://agentclientprotocol.com/get-started/architecture
- https://agentclientprotocol.com/get-started/clients
- https://agentclientprotocol.com/get-started/agents
- https://agentclientprotocol.com/protocol/v2/migration
- https://agentclientprotocol.com/rfds/v2/client-filesystem-terminal-capabilities
- https://agentclientprotocol.com/rfds/v2/terminal-output
- https://agentclientprotocol.com/llms.txt

### Hosts

- `zed-industries/zed` `crates/agent_servers/src/acp.rs` `client_capabilities_for_agent` (main, fetched 2026-08-25)
- https://github.com/zed-industries/zed/issues/43819 (Zed 0.214.7 initialize payload)
- https://github.com/zed-industries/zed/issues/57571 (Cursor `_meta.parameterizedModelPicker`)
- https://zed.dev/docs/ai/external-agents
- https://www.jetbrains.com/help/ai-assistant/acp.html
- `formulahendry/vscode-acp` `src/core/ConnectionManager.ts`, `src/handlers/FileSystemHandler.ts`
- `openclaw/acpx` `src/acp/client.ts` `resolveClientCapabilities`; `docs/CLI.md`; https://acpx.sh/permissions.html
- `getpaseo/paseo` `packages/server/src/server/agent/providers/acp-agent.ts` `BASE_ACP_CLIENT_CAPABILITIES`; `docs/custom-providers.md`; PR https://github.com/getpaseo/paseo/pull/2024
- `agentclientprotocol/python-sdk` `examples/gemini.py`

### Agents

- https://www.kimi.com/code/docs/en/kimi-code-cli/reference/kimi-acp.html (stale vs 0.37 on terminal)
- MoonshotAI/kimi-cli `src/kimi_cli/acp/AGENTS.md`, `tools.py` `replace_tools`, `kaos.py` `ACPKaos`
- https://github.com/MoonshotAI/kimi-cli/issues/1517
- Kimi Code 0.37 embedded `acpTerminalRunner.ts` / `acpFsService.ts` (local binary + Kivio probe 2026-08-25)
- https://github.com/multica-ai/multica/pull/7323
- google-gemini/gemini-cli `docs/cli/acp-mode.md`, `packages/cli/src/acp/acpClient.ts`, `acpFileSystemService.ts`, `packages/core/src/tools/glob.ts`
- https://opencode.ai/docs/acp/ ; `anomalyco/opencode` `packages/opencode/src/acp/agent.ts` ; OpenVibeCoding `docs/opencode-acp-integration-memo.md`
- https://cursor.com/docs/cli/acp
- NousResearch/hermes-agent `acp_adapter/tools.py`; issue https://github.com/NousResearch/hermes-agent/issues/569 ; https://hermes-agent.nousresearch.com/docs/developer-guide/acp-internals
- can1357/oh-my-pi PR https://github.com/can1357/oh-my-pi/pull/4335 ; `packages/coding-agent/src/tools/bash.ts` `wrapShellLineForClientTerminal`
- phuryn/grok-build-vscode `docs/internal/ACP-feedback.md`

### Kivio

- `src-tauri/src/external_agents/session/acp.rs` `acp_initialize_params`
- `src-tauri/src/external_agents/session/acp_terminal.rs`
- `src-tauri/src/external_agents/defs/acp.rs` (cursor / gemini / opencode / hermes / kimi)
- [ADR-0003](../adr/0003-acp-agents-import-via-protocol.md)
