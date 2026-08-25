# How Paseo and AionUi/AionCore adapt ACP hosts to many local CLIs

**Date:** 2026-08-25  
**Clones (shallow, do not treat as live remotes):**

| Repo | Path | HEAD |
| --- | --- | --- |
| getpaseo/paseo | `E:\ZM database\_acp-host-refs\paseo` | `54299a2` |
| iOfficeAI/AionUi | `E:\ZM database\_acp-host-refs\AionUi` | `16589d8` |
| iOfficeAI/AionCore | `E:\ZM database\_acp-host-refs\AionCore` | `3f5c9f9` |

**Kivio (this repo, read-only except this note):** `src-tauri/src/external_agents/session/acp.rs` (`acp_initialize_params`), `acp_terminal.rs`. Background: [acp-host-adaptation.md](./acp-host-adaptation.md).

AionUi is the Electron **UI**. The ACP **client/host** (initialize + reverse RPC) lives in AionCore, bundled as `aioncore`. `AionUi/packages/shared-scripts/src/prepare-aioncore.js` sets `GITHUB_OWNER = 'iOfficeAI'` and `GITHUB_REPO = 'AionCore'`. Do not treat the Electron tree as the host.

---

## One-page verdict for Kivio

Kivio’s live handshake (`terminal: true`, no `fs/*`) is **the same cell AionCore already ships**, not an accident unique to Kivio. Paseo is the opposite philosophy: **advertise nothing unless the operator opts in**, because advertising can replace a CLI’s local backend — especially when the agent and host do not share a filesystem (containers / remote daemons).

**Copy from Paseo**

1. Treat `clientCapabilities` as a **per-provider policy**, not a global “be like Zed.” Default false; opt in only when the host will actually execute. `BASE_ACP_CLIENT_CAPABILITIES` + `providerParams.clientCapabilities` (`paseo/packages/server/src/server/agent/providers/acp-agent.ts` `buildACPClientCapabilities`, `generic-acp-agent.ts` schema).
2. **Do not put `terminal: true` on the Kimi catalog row** hoping glob/grep recover. Paseo’s Kimi catalog entry is `command: ["kimi", "acp"]` with **no** `params.clientCapabilities` (`paseo/packages/app/src/data/acp-provider-catalog.ts`). The Kimi TypeScript subclass only probes per-model thinking options (`kimi-acp-agent.ts` `resolveKimiCatalogModels`).
3. Keep probe/import from owing reverse RPC you will not serve. Paseo’s probe **advertises the same flags as live** (including an override), but `buildProbeClient.createTerminal()` **throws**; live `ACPAgentSession.createTerminal` actually spawns. Kivio already splits advertise (`acp_initialize_params(true|false)`). Keep that split; if you ever opt in `fs` on live, still leave it off on probe unless the probe implements `fs/*`.
4. Session-level **Auto Accept** for `session/request_permission`, not a silent YOLO on `terminal/create`. Paseo auto-selects an allow option; chooser-shaped option lists stay interactive (`requestPermission` + `isACPChooserRequest`). It does **not** gate `terminal/create`.
5. Always send `cwd` + `mcpServers` on `session/new` **and** `session/load` even when the array is empty (Devin “Invalid params” comment on `initializeResumedSession`).
6. Cursor-only `_meta.parameterizedModelPicker: true` (`cursor-acp-agent.ts` `CURSOR_CLIENT_CAPABILITY_META`). Do not invent other per-CLI `_meta` until measured.

**Copy from AionCore / AionUi**

1. **Live path is already Kivio-shaped:** `terminal: true`, `fs.readTextFile` / `writeTextFile` **false** (comment: “fs stays undeclared (P2)”), plus `session.configOptions` so gated agents still send pickers (`AionCore/crates/aionui-ai-agent/src/protocol/acp.rs` `build_initialize_request`). They did **not** special-case Kimi.
2. Implement every advertised `terminal/*` method. AionCore’s `TerminalRegistry` is the real exec plane; AionUi only renders `MessageAcpTerminalOutput` and a per-command Stop (`killTerminal`).
3. Empty-`args` shell fallback for agents that send a compound **line** as `command` (codebuddy live 2026-08-05). Spec is still execvp when `args` is present. Kivio is execvp-only today — keep that for Kimi `bash -c`, add shell **only** when `args` is empty.
4. Permission **UI** for `session/request_permission`. AionCore’s router auto-approves **team MCP only**; everything else is a card. `terminal/create` itself is **not** permission-gated — codebuddy’s Bash card is a separate `request_permission`. YOLO is `session/set_mode` to a catalog `yolo_id`, not a host skip of reverse RPC.
5. Pass configured `mcpServers` on `session/new` **and** `session/load`. Shipping AionCore omits the field when the list is empty (`acp_assembler.rs` `new_session_request`); the unfinished `acp_conn` path always sends the array so resume cannot regress to `[]`. Paseo always sends the key (Devin). Kivio already always sends the key.

**Do not copy**

- Paseo’s **default `terminal: false` on live desktop sessions** if Kimi 0.37 Bash must work. Turning the flag off does not restore TUI glob; Kimi throws `ACP terminal capability is unavailable`.
- Paseo’s **OpenCode native HTTP adapter**. Paseo OpenCode is *not* ACP (`opencode-agent.ts`). Kivio/AionCore drive OpenCode as `opencode acp`.
- AionCore’s **unfinished** `aionui-session` ACP backend (`acp_conn.rs` `initialize_params`): advertises **fs false and no terminal**, then `-32601`s `fs/*` and `terminal/*`. Comment says opencode/gemini/hermes still use the **legacy** `AgentInstance::Acp` path (`AcpProtocol`). Do not treat `acp_conn` as shipping policy.
- AionCore probe **reusing live initialize** (`terminal: true` even on try-connect). Kivio’s probe `terminal: false` is better.
- Inventing glob/grep RPCs. Neither host has them. Paseo’s catalog never sets `terminal: true` for Kimi to “fix” search.
- Host-side `sh -c` wrapping of argv that already has `args` (pi/Kimi bash already wrap). Spec + Paseo: args present → execvp.
- Advertising `fs` before implementing it. AionCore explicitly deferred that to P2; Paseo only turns it on via operator params.

**Kimi 0.37 Glob/Grep:** neither host has a host-side fix. Paseo avoids the failure by **not advertising terminal** (Bash then also dies unless Kimi falls back — 0.37 does not). AionCore advertises terminal globally (Bash works, glob/grep still bash-gated). Kivio is in the AionCore cell. Next Kivio step is honesty (agent-policy errors, comment fix) + optional `fs/*` later for Gemini-class buffer sync — **not** a Kimi glob emulator.

---

## Paseo deep dive

### Architecture

Paseo is a daemon + app that drives **many coding CLIs**. Native (non-ACP) adapters own Claude stream-json, Codex app-server, OpenCode HTTP, Pi RPC, OMP. ACP is a **second family**.

**ACP client lives in** `paseo/packages/server/src/server/agent/providers/acp-agent.ts`:

- `ACPAgentClient` — spawn, `initialize`, catalog probe, session factory.
- `ACPAgentSession` — live session; implements the ACP SDK **Client** (reverse RPC).
- `GenericACPAgentClient` — `extends: "acp"` custom providers (`generic-acp-agent.ts`).
- Thin subclasses: `CursorACPAgentClient`, `KimiACPAgentClient`, `KiroACPAgentClient`, `TraeACPAgentClient`, plus first-class `CopilotACPAgentClient`.

**Factory wiring** (`provider-registry.ts` `PROVIDER_CLIENT_FACTORIES` + `createBaseClient` for `extends: "acp"`):

| Provider id | Client | Notes |
| --- | --- | --- |
| `cursor` | `CursorACPAgentClient` | Built-in factory **and** ACP-extend special case. Default command `["cursor-agent", "acp"]`. |
| `kimi` | `KimiACPAgentClient` | **Only** via custom/`extends: "acp"` with `providerId === "kimi"`. Not in `PROVIDER_CLIENT_FACTORIES`. |
| `kiro`, `traecli` | dedicated subclasses | Slash-command wait / Kiro extension notify. |
| everything else ACP | `GenericACPAgentClient` | Gemini, Hermes, Grok, Codebuddy, … |
| `opencode` | `OpenCodeAgentClient` | **Not ACP.** Native HTTP. |
| `copilot` | `CopilotACPAgentClient` | First-class ACP. |

Catalog UI: `paseo/packages/app/src/data/acp-provider-catalog.ts` (`ACP_PROVIDER_CATALOG`). Installing a row writes `extends: "acp"` + `command` (+ optional `params` / `env`).

Built-in **non-catalog** providers (`packages/protocol/src/provider-manifest.ts` `AGENT_PROVIDER_DEFINITIONS`): claude, codex, copilot, opencode, pi, omp. Gemini / Hermes / Kimi are **catalog ACP**, not native.

### Initialize payload

Default (still true at HEAD `54299a2`; CHANGELOG 0.1.107 cites PR [#2024](https://github.com/getpaseo/paseo/pull/2024)):

```246:268:paseo/packages/server/src/server/agent/providers/acp-agent.ts
const BASE_ACP_CLIENT_CAPABILITIES: ACPClientCapabilities = {
  fs: {
    readTextFile: false,
    writeTextFile: false,
  },
  terminal: false,
};

export function buildACPClientCapabilities(
  meta?: ACPClientCapabilityMeta,
  override?: ACPClientCapabilities,
): ACPClientCapabilities {
  const capabilities: ACPClientCapabilities = {
    ...BASE_ACP_CLIENT_CAPABILITIES,
    ...override,
    fs: {
      ...BASE_ACP_CLIENT_CAPABILITIES.fs,
      ...override?.fs,
    },
  };
  return meta && Object.keys(meta).length > 0 ? { ...capabilities, _meta: meta } : capabilities;
}
```

Handshake (probe **and** live):

```1195:1201:paseo/packages/server/src/server/agent/providers/acp-agent.ts
          transport.connection.initialize({
            protocolVersion: PROTOCOL_VERSION,
            clientCapabilities: buildACPClientCapabilities(
              this.clientCapabilityMeta,
              this.clientCapabilities,
            ),
            clientInfo: { name: "Paseo", version: "dev" },
```

Same object in live `spawnProcess` (`connection.initialize` ~2526–2534).

**Not advertised unless override / meta:** `auth.terminal`, `elicitation`, session `configOptions`. Cursor injects `_meta` only:

```16:18:paseo/packages/server/src/server/agent/providers/cursor-acp-agent.ts
const CURSOR_CLIENT_CAPABILITY_META = {
  parameterizedModelPicker: true,
};
```

**Per-provider override** is generic ACP `params`, not a hardcoded Kimi flag:

```24:34:paseo/packages/server/src/server/agent/providers/generic-acp-agent.ts
    clientCapabilities: z
      .object({
        fs: z
          .object({
            readTextFile: z.boolean().optional(),
            writeTextFile: z.boolean().optional(),
          })
          .optional(),
        terminal: z.boolean().optional(),
      })
      .optional(),
```

Passed through as `clientCapabilities: providerParams.clientCapabilities` into `ACPAgentClient`.

Unit test documents the post-#2024 contract (`acp-agent.test.ts` `buildACPClientCapabilities`):

> “keeps filesystem and terminal execution with the agent by default”

Override merge: `readTextFile: true` + `terminal: true` leaves `writeTextFile: false` unless set. `_meta` is preserved.

Docs (`paseo/docs/custom-providers.md`): “ACP agents execute filesystem and terminal operations in their own environment by default. To let a compliant agent delegate those operations to Paseo instead, enable the corresponding client capabilities.” Warning: only enable what Paseo should execute; remote/container path mismatch is the reason.

CHANGELOG 0.1.107: “Custom ACP providers keep file and terminal work in the agent environment by default ([#2024](https://github.com/getpaseo/paseo/pull/2024)).” Current code **still** defaults false.

### Catalog JSON never sets `terminal: true` for Kimi

`paseo/packages/app/src/data/acp-provider-catalog.ts` `CATALOG_DATA`:

```256:263:paseo/packages/app/src/data/acp-provider-catalog.ts
    id: "kimi",
    title: "Kimi Code CLI",
    description: "Moonshot AI's open-source terminal coding agent",
    version: "0.11.0",
    iconId: "kimi",
    installLink: "https://github.com/MoonshotAI/kimi-code",
    command: ["kimi", "acp"],
```

No `params` key. The **only** catalog `params` in the whole file is Factory Droid:

```165:170:paseo/packages/app/src/data/acp-provider-catalog.ts
    command: ["npx", "-y", "droid@0.179.0", "exec", "--output-format", "acp-daemon"],
    env: {
      DROID_DISABLE_AUTO_UPDATE: "true",
      FACTORY_DROID_AUTO_UPDATE_ENABLED: "false",
    },
    params: { supportsMcpServers: false },
```

Gemini (`npx -y @google/gemini-cli@… --acp`), Hermes (`hermes acp`), Cursor (`cursor-agent acp`) also have **no** `clientCapabilities`. Grok is catalog ACP (`grok agent stdio`) with no capability override either.

So: installing Kimi from the Paseo catalog yields **fs false, terminal false**. Kimi 0.37 then fails closed on **all** `AcpProcessService` spawn (Bash + Glob/Grep). That is a deliberate “keep I/O in the agent environment” tradeoff for a product that often runs agents in **other** environments than the GUI.

### Reverse RPC — live vs probe

**Probe client** (`ACPAgentClient.buildProbeClient`):

```1214:1232:paseo/packages/server/src/server/agent/providers/acp-agent.ts
  protected buildProbeClient(): ACPClient {
    return {
      async requestPermission(): Promise<RequestPermissionResponse> {
        return { outcome: { outcome: "cancelled" } };
      },
      async sessionUpdate(): Promise<void> {},
      async readTextFile(params: ReadTextFileRequest) {
        const content = await fs.readFile(params.path, "utf8");
        return { content };
      },
      async writeTextFile(params: WriteTextFileRequest) {
        await fs.mkdir(path.dirname(params.path), { recursive: true });
        await fs.writeFile(params.path, params.content, "utf8");
        return {};
      },
      async createTerminal() {
        throw new Error("ACP model probe does not support terminal execution");
      },
    };
  }
```

Probe `session/new` uses `mcpServers: []` (`fetchCatalog` / diagnostics ~973–1033, ~1306–1309).

**Live session** (`ACPAgentSession`) implements the full Client:

| Method | Function | Behavior |
| --- | --- | --- |
| `session/request_permission` | `requestPermission` ~2233 | Auto-accept if feature on and not a chooser; else UI event. |
| `fs/read_text_file` | `readTextFile` ~2368 | Local `fs.readFile`; honors `line`/`limit` (1-based). **No cwd jail.** |
| `fs/write_text_file` | `writeTextFile` ~2379 | `mkdir` + write. **No cwd jail.** |
| `terminal/create` | `createTerminal` ~2385 | Spawn (see below). |
| `terminal/output` | `terminalOutput` ~2444 | Buffer + truncate + exit. |
| `terminal/wait_for_exit` | `waitForTerminalExit` ~2453 | Promise on child exit. |
| `terminal/kill` | `killTerminal` ~2466 | `terminateProcess`. |
| `terminal/release` | `releaseTerminal` ~2458 | Kill if still running, then drop. |
| elicitation | — | **Not implemented** on ACP Client. Codex app-server has MCP elicitation separately. |
| `auth.terminal` | — | **Not advertised.** |

**Terminal spawn** (`resolveTerminalCommand` + `createTerminal`):

```199:213:paseo/packages/server/src/server/agent/providers/acp-agent.ts
function resolveTerminalCommand(
  command: string,
  args?: string[],
): { command: string; args: string[]; shell?: boolean } {
  if (args && args.length > 0) {
    return { command, args };
  }
  if (!/\s/.test(command.trim())) {
    return { command, args: [] };
  }
  const shell = buildStringCommandShellInvocation({ command, windowsShell: "cmd" });
  return { command: shell.shell, args: shell.args, shell: false };
}
```

- `args` present → **execvp** (`command` + `args`).
- Bare executable → execvp, empty args.
- Whitespace in `command` and no args → **bash `-c` / `cmd.exe /c`** via `buildStringCommandShellInvocation` (`string-command-shell.ts`). `shell: false` on `spawnProcess` so Node does not double-wrap; overlay clears `BASH_ENV`.
- `cwd`: `params.cwd ?? this.config.cwd`. **No extra-root confinement.**
- Env: agent `params.env` plus `createProviderEnvSpec`. **No forced `TERM=dumb` / `NO_COLOR=1`** (Kivio does set those).

**Why probe implements fs but default-advertises false:** methods exist if an agent ignores the spec and calls anyway; `createTerminal` is explicitly refused so a catalog probe cannot launch user commands. Diagnostic test `sends configured client capabilities in catalog and live session initialization` (`generic-acp-agent.diagnostic.test.ts`) proves **catalog probe and live session send the same** `clientCapabilities` when `providerParams` opts in — including `terminal: true`. Opting in then probing is a footgun: advertise true, `createTerminal` throws.

### Kimi / Cursor / Gemini / OpenCode / Hermes

**Kimi** (`kimi-acp-agent.ts`): subclass **only** for catalog thinking-option probing (`resolveKimiCatalogModels` walks models via `setSessionConfigOption`). Comment: “This lives on the Kimi client, not the base ACP adapter.” **No spawn/terminal/fs override.** Smoke/catalog command `["kimi", "acp"]`.

**Cursor:** `_meta.parameterizedModelPicker`; wait 10s for `available_commands_update`; Fast feature mapped to ACP config `fast`. Smoke test (`cursor-acp-smoke.test.ts`): “cursor-acp@0.1.0 emits no requestPermission calls; Cursor's hidden TUI permission UI is out of scope.” So Cursor often **does not** use host permission or host terminal.

**Gemini:** catalog generic ACP. Docs example `["gemini", "--acp"]` with **no** capability opt-in (`custom-providers.md`). Glob/grep stay in-agent if Gemini’s own policy is followed; Paseo does not special-case them.

**OpenCode:** **not ACP in Paseo.** `OpenCodeAgentClient` + `auto_accept` feature for OpenCode’s own permission protocol. Do not infer OpenCode ACP glob/grep from Paseo.

**Hermes:** catalog `["hermes", "acp"]`, generic client (`generic-acp-agent.test.ts` uses hermes as the example command). No capability override, no yolo mode in Paseo’s ACP auto-accept beyond the shared `auto_accept` toggle.

**Copilot:** first-class ACP; `allow-all` mode is Copilot’s own unattended mode, not `clientCapabilities.terminal`.

### Permission / YOLO / Plan

- Shared ACP feature `auto_accept` (`ACP_AUTO_ACCEPT_FEATURE_ID`). Unattended creates default it on (`resolveACPCreateConfig`).
- `requestPermission`: if auto-accept **and** not `isACPChooserRequest`, pick first allow-kind option. Chooser = two options sharing the same allow kind (AskUser-style). “does not auto-accept ACP chooser requests” (`acp-agent.test.ts`).
- If auto-accept but **no** allow option → still surface to the user.
- **Plan** for ACP is whatever mode the agent reports (`session/set_mode` / config options). Paseo does **not** refuse `terminal/create` in plan mode. Codex plan is a **different** native adapter.
- `terminal/create` is **not** routed through `requestPermission`. Host exec happens as soon as the agent calls it (if the capability was advertised). Default advertise-false means Kimi never gets here.

### MCP

Live: `acpMcpServers()` → `normalizeMcpServers(this.config.mcpServers)` if `capabilities.supportsMcpServers` (default **true** on `DEFAULT_ACP_CAPABILITIES`; generic params may set `supportsMcpServers: false` — Factory Droid catalog).

```2548:2549:paseo/packages/server/src/server/agent/providers/acp-agent.ts
  private acpMcpServers(): McpServer[] {
    return this.capabilities.supportsMcpServers ? normalizeMcpServers(this.config.mcpServers) : [];
```

`session/new` and `session/load` / resume always include `mcpServers` (possibly `[]`). Comment on `initializeResumedSession`: Devin requires all three of `sessionId`, `cwd`, `mcpServers`.

Probe/diagnostics: `mcpServers: []`.

Docs: some ACP adapters cannot create sessions when `mcpServers` is non-empty → `params.supportsMcpServers: false`.

### What Paseo chose not to do

- Not Zed-style always-on fs+terminal (explicitly flipped in #2024; docs + `BASE_ACP_CLIENT_CAPABILITIES` + tests still say so).
- No per-Kimi `terminal: true` to rescue Bash.
- No glob/grep Client methods.
- No ACP elicitation Client.
- No `auth.terminal` in initialize.
- No cwd jail on fs/terminal (docs tell operators to align paths instead).
- OpenCode kept on native protocol rather than ACP.
- Probe cancels all permissions; probe refuses terminals even if advertised.

---

## AionUi + AionCore deep dive

### Architecture

**AionUi** (`16589d8`): Electron renderer + thin process layer. ACP types, permission cards, terminal cards, agent settings. Does **not** speak JSON-RPC to CLIs.

**AionCore** (`3f5c9f9`): Rust host. Two ACP stacks:

1. **Shipping (legacy `AgentInstance::Acp`)** — `crates/aionui-ai-agent/src/protocol/acp.rs` `AcpProtocol` using crate `agent-client-protocol` 2.0. Comment in `aionui-session/src/backend/mod.rs` `pending_permission_requests`: “today only claude/codex are [on SessionBackend]; **opencode/gemini/hermes still use the legacy `AgentInstance::Acp` path**.” Kimi/cursor/codebuddy go through the same ACP manager (`manager/acp/`).
2. **Clean-slate `AcpConnection`** — `crates/aionui-session/src/backend/acp_conn.rs`. SDK-free JSON-RPC. **Not** the live path for those CLIs yet.

Builtin catalog is SQLite seed, not a TS file: `crates/aionui-db/migrations/001_initial_schema.sql` plus later syncs (`025_sync_and_add_acp_registry_agents.sql`, pi/omp/mimo, …). AionUi `docs/prds/conversations/acp/agent-skill-discovery.md` lists the builtin ACP backends (claude, codex, gemini, qwen, codebuddy, kimi, opencode, cursor, hermes, …).

### Initialize payload (shipping `AcpProtocol`)

```96:113:AionCore/crates/aionui-ai-agent/src/protocol/acp.rs
fn build_initialize_request() -> InitializeRequest {
    // Advertised client services:
    // - terminal: full `terminal/*` suite backed by TerminalRegistry —
    //   delegated commands run in OUR process tree (live output, per-command
    //   kill, audit logging). Adoption probed 2026-08-05: codebuddy/grok/omp.
    // - session.config_options: we already consume `config_option_update`
    //   and render the options UI; declaring it stops strictly-capability-
    //   gated agents from withholding their config options.
    // fs stays undeclared (P2).
    let mut session_caps = ClientSessionCapabilities::default();
    session_caps.config_options = Some(SessionConfigOptionsCapabilities::default());
    let mut caps = ClientCapabilities::default();
    caps.terminal = true;
    caps.session = Some(session_caps);
    InitializeRequest::new(ProtocolVersion::LATEST)
        .client_info(Implementation::new(ACP_CLIENT_NAME, ACP_CLIENT_VERSION))
        .client_capabilities(caps)
}
```

`clientInfo`: name `"AionUi"`, version `CARGO_PKG_VERSION` (Mistral Vibe rejects empty client metadata, issue #3326).

Test `initialize_request_declares_terminal_and_config_options_capabilities` (~1594–1604):

- `clientCapabilities.terminal` **true**
- `session.configOptions` present
- `fs.readTextFile` / `writeTextFile` **false**

**No per-agent override.** Same payload for kimi, cursor, gemini, opencode, hermes, codebuddy.

**Not advertised:** `auth.terminal`, elicitation, `fs`. Auth **methods** from the **agent** are stored in `agent_metadata.auth_methods` (Kimi `_meta.terminal-auth` for `kimi login` — that is **agent** capability metadata, not Client `auth.terminal`).

CHANGELOG (AionUi 2.1.50 / AionCore #779): “client-hosted terminals — declare clientCapabilities.terminal and serve terminal/*”. That is when they **became** Kivio-shaped.

### Unfinished `acp_conn` (do not confuse with live)

```162:168:AionCore/crates/aionui-session/src/backend/acp_conn.rs
/// `initialize` params (ACP). `protocolVersion` + client-side capabilities; we
/// advertise the reverse-RPC we actually handle (`session/request_permission`).
fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } }
    })
}
```

No `terminal: true`. Reverse RPC: `session/request_permission` surfaced; **any other method including `fs/*` and `terminal/*` → `-32601`** (`handle_reverse_rpc` ~1544, default arm ~1610). Comment: unhandled reverse RPC must be clean-rejected, not dropped. Elicitation: “0/24 live agents emit it — 2026-08-04 sweep” (`Command::AnswerAsk`).

This is the **Paseo-like conservative handshake** sitting next to the **shipping Zed-lite terminal-only** handshake. Kivio should follow **shipping `AcpProtocol`**, not `acp_conn`, until AionCore actually migrates Gemini/Kimi onto it.

### Reverse RPC (shipping)

`AcpProtocol::connect` registers SDK handlers (`acp.rs` ~699–758):

| Method | Handler | Notes |
| --- | --- | --- |
| `session/request_permission` | `handle_permission_request` | Forwards to `PermissionRouter`; wait for user / auto team-MCP. |
| `terminal/create` | `handle_terminal_create` | Spawn immediately; start UI snapshot poller. **No permission check here.** |
| `terminal/output` | `handle_terminal_output` | Registry snapshot. |
| `terminal/wait_for_exit` | `handle_terminal_wait` | |
| `terminal/kill` | `handle_terminal_kill` | User Stop button → `ipcBridge.conversation.killTerminal`. |
| `terminal/release` | `handle_terminal_release` | |
| `fs/read_text_file` / `write_text_file` | **not registered** | SDK default-reject. Matches advertised false. |
| elicitation | **not registered** | Unimplemented on purpose. |

**`TerminalRegistry::create`** (`terminal.rs` ~113–138):

> Agents disagree on the `command` contract: grok wraps itself in `/bin/bash -lc '<line>'` (command + args), while codebuddy sends the whole compound line — `sleep 2 && echo hi` — as a bare `command` with no args, expecting shell interpretation (live-captured 2026-08-05: direct exec ENOENTs on the compound string). Rule: **explicit args → exec verbatim; no args → run the line through the platform shell.**

Unix: `/bin/sh -c <command>`. Windows: `cmd /C <command>`. Then:

- stdin null, stdout/stderr piped
- agent `env` applied as given (**no** `TERM=dumb` / `NO_COLOR` injection)
- cwd: `params.cwd` or conversation workspace `default_cwd` (never inherit aioncore’s app-bundle cwd)
- **no workspace jail**
- output cap 1 MiB default, **truncate from the front** at a char boundary (spec MUST)
- kill/reap: poll `try_wait` because `sh -c 'sleep 30 &'` exits while pipes stay open

AionUi card: `packages/desktop/src/renderer/pages/conversation/Messages/acp/MessageAcpTerminalOutput.tsx` — live output, Stop calls `killTerminal`. E2E `tests/e2e/features/conversations/acp/terminal-card.e2e.ts`: **codebuddy** delegates via `terminal/create` when the client declares the capability; test auto-clicks permission `allow` / `allow_always` **in parallel** while waiting for the card. Sleep-as-command was observed to **bypass** `terminal/create` (codebuddy background-task tool); a ticking loop stays on the delegated terminal.

### Probe vs live (AionCore)

`protocol/custom_agent_probe.rs` `run_handshake` calls the **same** `AcpProtocol::connect` → **same** `build_initialize_request` → **`terminal: true` on probes**. Probe then `session/new` with `temp_dir()`, no prompt. Terminal handlers are live; a misbehaving agent could spawn during try-connect. Kivio’s probe `terminal: false` is stricter.

Probe channels for events/permissions are throwaway (`// a probe session never sends a prompt`).

### Kimi / Cursor / Gemini / OpenCode / Hermes (AionCore catalog)

Seed `001_initial_schema.sql` (commands later patched by 025):

| backend | command / args (after 025) | native_skills_dirs | yolo_id after migrations |
| --- | --- | --- | --- |
| `kimi` | `kimi` `["acp"]` | `.kimi/skills` | **NULL** (025 cleared goose/auggie/kimi/copilot) |
| `cursor` | `cursor-agent` `["acp"]` | `.cursor/skills` | **NULL** |
| `gemini` | `gemini` `["--acp"]` | `.gemini/skills` | still `'yolo'` in seed; 025 only changed args |
| `opencode` | `opencode` `["acp"]` | `.opencode/skills` | `'build'` (unattended alias, not ACP terminal) |
| `hermes` | `hermes` `["acp"]` | NULL (prompt-inject skills) | **NULL** (`010_hermes_yolo_id_null.sql`) |
| `codebuddy` | npx `codebuddy --acp` | `.codebuddy/skills` | `bypassPermissions` |

Kimi auth row (`003_agent_acp_capabilities.sql`): agent `auth_methods` with `_meta.terminal-auth.command = "kimi"`, args `["login"]`. That is **login-in-a-terminal UX**, not `clientCapabilities.auth.terminal` and not `terminal/create` for tools.

**No kimi-specific clientCapabilities, terminal flags, or glob comments** in AionCore `terminal.rs` / `protocol/acp.rs`. Adoption comment lists **codebuddy / grok / omp**, not Kimi.

AionUi `mobile/src/constants/agentModes.ts`: Cursor agent/plan/ask via `session/set_mode`; OpenCode plan/build **no yolo**; Gemini default/autoEdit/yolo (**auto-approve at manager layer, not via ACP** — this comment is about Gemini **modes**, and the shipping `PermissionRouter` only auto-approves **team MCP**, so Gemini yolo is `session/set_mode` to the CLI’s yolo, not host skip of `request_permission`). Hermes has no modes entry.

### Permission / YOLO / Plan

`manager/acp/permission_router.rs`: auto-approve **only** if MCP server name is the team server; pick `AllowAlways` else `AllowOnce`. All other `session/request_permission` → `AgentStreamEvent::AcpPermission` → AionUi `MessageAcpPermission.tsx`.

YOLO: `mode_normalize.rs` maps UI aliases `yolo` / `yoloNoSandbox` to `metadata.yolo_id` when set. Kimi/cursor/hermes **have no yolo_id** after migrations → alias passes through; if the CLI has no such mode, the picker should not offer it (PRD: Codex/Gemini “generic” backends omit YOLO — the PRD is slightly stale vs Gemini seed `yolo_id`).

Plan: Cursor `plan` / OpenCode `plan` via `session/set_mode`. **Host does not block `terminal/create` in plan.** Grok was listed as a terminal adopter; Plan + host exec is the same hazard as in [acp-host-adaptation.md](./acp-host-adaptation.md).

`terminal/create` is **not** gated by the permission router. If the agent asks permission first, the user sees a card; if it only calls `terminal/create`, the process starts.

F-FILE-02 in AionUi PRD (“AI 读取和写入文件”) is **agent-local tools rendered as tool cards**, not ACP `fs/*` (those are unimplemented).

### MCP

Shipping `factory/acp.rs` loads DB MCP so they reach `session/new` (comment ELECTRON-1JG: without this the agent starts with zero MCP tools). `acp_assembler.rs` `new_session_request` **omits** `mcpServers` when the list is empty. Live `agent_session_flow.rs` attaches MCP on `session/new` and `session/load` **only if non-empty**. The unfinished `acp_conn.rs` `load_session_params` always sends `mcpServers` (comment: resume re-injects; pre-0c `mcpServers: []` “silently loses every MCP tool”). Custom probe `NewSessionRequest::new(temp_dir())` — **no extra MCP**.

`025` / custom-providers analogue: some agents cannot take MCP (Paseo Factory Droid). AionCore filters by agent-advertised `mcp_capabilities` in `load_user_mcp_servers`.

### What they chose not to do

- **fs Client methods** (“P2”). Advertising false + not registering handlers.
- Per-Kimi capability fork.
- Elicitation (measured 0/24 agents, 2026-08-04).
- Cwd confinement on host terminals.
- Forced `TERM`/`NO_COLOR` (Kivio **does**; AionCore relies on the agent’s env — Kimi’s bash gate requires the **agent** to send those env vars on `terminal/create`).
- Gating `terminal/create` on Plan/YOLO.
- Using `acp_conn`’s conservative initialize for shipping CLIs (yet).
- AionUi PRD skip list: Gemini-specific E2E skipped by user request; codebuddy YOLO id inconsistency noted as “待研发确认”.

---

## Side-by-side

| | **Paseo** (daemon ACP) | **AionCore shipping `AcpProtocol`** | **Kivio live** | **Kivio probe/import** |
| --- | --- | --- | --- | --- |
| **Advertise fs.read / fs.write** | false unless `params.clientCapabilities` | false (P2) | **omitted** (schema default false) | omitted |
| **Advertise terminal** | false unless params | **true** (all ACP CLIs) | **true** | **false** |
| **Advertise auth.terminal / elicitation** | no | no | no | no |
| **`_meta`** | Cursor `parameterizedModelPicker` only | no client `_meta` in initialize | no | no |
| **`session.configOptions` capability** | not in BASE object | **yes** | no | no |
| **Implement fs/** | yes, if called; default not advertised | no handlers | no (`-32601`) | n/a |
| **Implement terminal/** | yes live; probe **throws** create | yes (create/output/wait/kill/release) | yes | advertised false; methods not served |
| **exec vs shell** | args → execvp; no-args whitespace → bash/cmd | args → execvp; **no args → sh/cmd** | execvp always; `TERM=dumb` `NO_COLOR=1` | — |
| **cwd jail** | no (cwd = param or session) | no (param or workspace default) | no (`set_extra_roots` no-op) | — |
| **Kimi catalog** | `kimi acp`, **no** cap override | `kimi acp`, **same global terminal:true** | `kimi acp` family row | terminal false |
| **Kimi subclass** | thinking-option catalog only | none | none (shared ACP session) | — |
| **Glob/grep implication** | Kimi: process backend **not** replaced (terminal false) → 0.37 **fails all spawn** including Bash. Gemini/OpenCode N/A or in-agent. | Kimi: spawn **replaced**; Bash via host; Glob/Grep **pre-RPC bash gate**. Gemini/OpenCode: in-agent glob/grep; codebuddy Bash via host. | same as AionCore for Kimi | Kimi Bash+glob die (`capability unavailable`) |
| **OpenCode** | **native HTTP**, not ACP | **`opencode acp`** | **`opencode acp`** | — |
| **Permission** | Auto Accept feature; choosers stay UI; **no** terminal/create gate | team-MCP auto; else UI card; **no** terminal/create gate | `choose_permission_outcome` **always** allow_once/always | cancelled / unused |
| **YOLO / Plan** | ACP auto_accept toggle; plan = agent mode | `yolo_id` → `session/set_mode`; kimi/cursor/hermes yolo_id NULL | no ACP mode YOLO; auto-allow permissions | — |
| **mcpServers on session/new** | always send key (or [] if `supportsMcpServers: false`) | user + team when non-empty; omit field if empty; `session/load` same | always send key (`build_session_new_params`) | `[]` on probe |

---

## Concrete Kivio next steps (from these two hosts)

1. **Keep live `terminal: true`.** AionCore shipping code is the existence proof that a multi-CLI desktop host chooses this globally (codebuddy/grok/omp need it). Paseo’s default-false is for **environment isolation**, not for Kimi glob. Copy Paseo’s **opt-in fs**, not its live terminal-off default.

2. **Do not add a Kimi-only `terminal: false` or `terminal: true` override.** Paseo catalog does not; AionCore does not. A Kimi-only true is what you already have globally. A Kimi-only false kills Bash.

3. **Do not advertise `fs` until `fs/read_text_file` + `fs/write_text_file` exist.** AionCore left a `P2` comment rather than advertising true. Paseo only turns flags on with implemented methods. When you add fs: absolute paths, `line`/`limit`, create-on-write, text only (Grok image lesson in the sibling note). Still omit on probe.

4. **Keep probe `terminal: false`.** Do **not** copy AionCore’s probe=live initialize. Do copy Paseo’s idea that a probe must not run user commands (`createTerminal` throw) — Kivio already avoids the advertise.

5. **Empty-args shell fallback only** (AionCore `TerminalRegistry::create`, Paseo `resolveTerminalCommand`). Do **not** wrap argv that already has `args` (Kimi `bash -c` + env). Do **not** hope a host shell will see Kimi Glob’s `fd` — 0.37 rejects before RPC.

6. **Keep `TERM=dumb` / `NO_COLOR=1` on host spawn** (Kivio already). AionCore does not inject them; Kimi’s gate is on the **agent-supplied** env. Harmless extra for other CLIs.

7. **Stop auto-allowing every `request_permission` as the only policy.** Paseo: Auto Accept toggle + chooser exception. AionCore: real cards, team-MCP auto only. Kivio `choose_permission_outcome` always picks allow — that is stricter YOLO than either host. Especially do not treat `terminal/create` as covered by Plan mode.

8. **Keep sending `mcpServers` (possibly `[]`) on new + load.** Paseo: Devin rejects omitted keys. AionCore shipping omits the field when empty; `acp_conn` learned not to resume with `[]`. Kivio `build_session_new_params` already always sends the key — keep that.

9. **Cursor `_meta.parameterizedModelPicker`** when Kivio’s picker can consume parameterized model ids (Paseo `CURSOR_CLIENT_CAPABILITY_META`). Harmless if Cursor ignores it; Zed already sends it.

10. **Declare `session.configOptions` if you already consume config-option updates.** AionCore added this so strict agents do not hide pickers. Cheap, not a glob fix.

11. **UI: live `{type:terminal}` / `terminal_id` cards** (AionUi `MessageAcpTerminalOutput` + kill). Spec SHOULD keep showing output after release. Kivio implements `terminal/*` but the chat UI should not pretend Glob failures are host RPC errors.

12. **Fix `acp_terminal.rs` comment** if it still implies all ACP CLIs route Glob/Grep through the host. AionCore’s own comment names **codebuddy/grok/omp**, not Kimi glob. OpenCode on both Kivio and AionCore is ACP-as-control-plane with **local** glob/grep.

13. **Do not wait on AionCore `acp_conn` or ACP v2.** v2 drops Client fs/terminal (sibling note). `acp_conn` currently advertises **less** than Kivio live. Stay on v1 `AcpProtocol`-class surface.

14. **OpenCode:** follow AionCore (`opencode acp`), not Paseo’s native HTTP adapter.

15. **Upstream Kimi** remains the glob fix (local `fd`/`rg` like kimi-cli / Gemini, or wrap as `bash -c`). Hosts in this survey did not paper over it.

---

## Kivio snapshot (for the table)

```153:165:src-tauri/src/external_agents/session/acp.rs
fn acp_initialize_params(terminal: bool) -> Value {
    // Live chat sessions must advertise `terminal: true` and implement `terminal/*`.
    // Probe/import processes never handle those methods, so they keep it false.
    json!({
        "protocolVersion": ACP_PROTOCOL_VERSION,
        "clientCapabilities": { "terminal": terminal },
        "clientInfo": {
            "name": "kivio",
            "title": "Kivio",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}
```

Live vs probe asserted in `live_handshake_advertises_terminal_capability`. `handle_agent_to_client_request`: `request_permission` auto-selects allow; `terminal/*` delegated to `AcpTerminalHost`; anything else `-32601`. `AcpTerminalHost::create`: `Command::new(command).args(args)`, session cwd, `TERM=dumb` `NO_COLOR=1`. Family defs: `src-tauri/src/external_agents/defs/acp.rs` (cursor / gemini / opencode / hermes / kimi).
