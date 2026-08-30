---
id: automation
name: automation
description: 创建/编辑/运行 Kivio 自动化工作流（定时、热键、Agent、通知、HTTP、命令、产出 PDF/Word）。Create, edit, and run Kivio automations. Use when the user asks to 创建自动化, 工作流, 定时任务, gather news on a schedule, or build a graph of triggers and actions. Activate this skill BEFORE calling automation_upsert — do not probe node types with dry_run.
recommended-tools:
  - automation_list
  - automation_get
  - automation_upsert
  - automation_set_enabled
  - automation_run
  - automation_runs
  - automation_delete
---

# Kivio automations

Kivio automations are directed graphs: one trigger, then actions and logic, stored as JSON.

**Do this:** activate this skill, `automation_list` once, then `automation_upsert` with a **complete** graph in one call.

**Do not:** glob the repo, invent node type names, or `dry_run` to discover the schema. `dry_run` is only for a finished candidate. This skill is the schema — `automation_upsert`'s tool description is short on purpose. Validation errors also return `allowedNodeTypes` + `schemaHint`.

Chat exposes seven native tools. After creating a graph, tell the user to open `#chat/automations/{id}`.

## Tools

1. `automation_list` — ids, names, enabled, trigger type. **Call this first** to avoid duplicates.
2. `automation_get` — full graph for one id.
3. `automation_upsert` — create or replace the whole graph in **one** call. Omit `id` to create. Omit node `position` to auto-layout. `dry_run: true` only after you already have a complete graph. Error-severity issues reject the save and return `allowedNodeTypes` + `schemaHint`; fix those and resubmit the whole graph.
4. `automation_set_enabled` — start/stop schedule and hotkey registration (separate from upsert).
5. `automation_run` — **blocking**. Optional `input` (string or `{ text, json }`) becomes `{{output}}` / `{{json.input.*}}` for later nodes. Default wait 600s (max 1800). Timeout is not a failure: the run continues; poll with `automation_runs`.
6. `automation_runs` — recent summaries, or one run's per-node details when `run_id` is set.
7. `automation_delete` — permanent. Goes through normal approval.

Do **not** call these tools from inside an `action.agent` step. Workflow agents never receive `automation_*` tools.

After creating or changing a graph, tell the user to open `#chat/automations/{id}` (Extensions → Automations) to inspect the canvas.

## Graph contract (schema v1)

```json
{
  "schemaVersion": 1,
  "id": "",
  "name": "Daily briefing",
  "enabled": false,
  "nodes": [],
  "edges": [],
  "viewport": { "x": 0, "y": 0, "zoom": 1 }
}
```

- Exactly one trigger recommended (`trigger.manual` | `trigger.schedule` | `trigger.hotkey`).
- Main-flow edges: `{ "id", "source", "target" }`. Tree only — no cycles.
- Agent slot edges (not part of the main flow): `source` is the slot node, `target` is `action.agent`, `sourceHandle`: `"slot"`, `targetHandle`: `"runtime"` | `"context"` | `"tool"` | `"skill"`.
- Node ids unique. Positions optional (auto-layout when every node is at `{x:0,y:0}`).
- `enabled: false` until the user asks to arm schedule/hotkey. Manual and agent runs still work.

## Interpolation

Downstream templates:

- `{{output}}` — previous node's text. After `automation_run` with a string input, this is that string at the trigger.
- `{{json.path}}` — walk the previous JSON. Agent-run input is nested at `json.input`.
- `{{json.input.topic}}` — a field from `automation_run` `{ "input": { "json": { "topic": "..." } } }`.

No `$json` expressions, no `$('Node')` cross-references.

## Node types and `data`

Every node: `{ "id", "type", "position"?, "data": { "label": "..." } }`. Extra fields live under `data`.

### Triggers

**`trigger.manual`** — editor / agent test runs only.

**`trigger.schedule`**

```json
"data": {
  "label": "Schedule",
  "schedule": { "kind": "daily", "hour": 9, "minute": 0, "intervalMinutes": 60 }
}
```

`kind`: `daily` | `weekdays` | `interval`. For `interval`, `intervalMinutes` ≥ 1. `hour` 0–23, `minute` 0–59.

**`trigger.hotkey`**

```json
"data": { "label": "Hotkey", "hotkey": { "accelerator": "CommandOrControl+Shift+B" } }
```

Modifiers: `CommandOrControl`, `Control`, `Alt`, `Shift`, `Super`.

### Actions

**`action.agent`** — unattended model step. Put prompt/runtime/tools/skills on **slot nodes**, not inline.

**`action.notify`** — `{ "notify": { "body": "{{output}}" } }`

**`action.http`** — `{ "http": { "method": "GET", "url": "https://…", "headers": "", "body": "" } }`  
`method`: GET|POST|PUT|PATCH|DELETE. URL required.

**`action.set`** — `{ "set": { "fields": [{ "key": "text", "value": "{{output}}" }] } }`

**`action.clipboard`** — `{ "clipboard": { "op": "copy", "text": "{{output}}" } }` (`copy` | `read`)

**`action.file`** — `{ "file": { "op": "write", "path": "notes.md", "content": "{{output}}" } }` (`read` | `write`; path required)

**`action.command`** — `{ "command": { "command": "echo hi", "cwd": "", "timeoutSeconds": 30, "continueOnFail": false } }`

### Logic

**`logic.if`** — `{ "if": { "op": "contains", "value": "ok" } }`  
`op`: `contains` | `equals` | `notEmpty`. Outgoing `sourceHandle`: `"true"` | `"false"`.

**`logic.switch`** — `{ "switch": { "cases": [{ "id": "1", "op": "equals", "value": "a" }] } }`  
Outgoing handles: each case `id`, plus `"default"`.

**`logic.delay`** — `{ "delay": { "seconds": 5 } }` (integer ≥ 1)

### Agent slots (plug into `action.agent`)

Slot node `data.agent`:

**`agent.runtime`**

```json
"agent": {
  "runtimeKind": "builtin",
  "providerId": "",
  "model": "",
  "externalAgentId": null,
  "externalModel": null
}
```

`runtimeKind`: `builtin` | `chat` | `external`. External needs `externalAgentId` (claude, codex, cursor, …).

**`agent.context`** — `{ "agent": { "prompt": "Summarize {{output}} in 5 bullets." } }`

**`agent.tool`** — `{ "agent": { "toolIds": ["web_search", "web_fetch"] } }`  
Empty = read-only tools + skill loader. Never include `automation_*` or memory tools.

**`agent.skill`** — `{ "agent": { "skillIds": ["pdf"] } }`

## Best practices

1. `automation_list` before create. Reuse an id with upsert to replace.
2. `dry_run: true` first. Fix every `severity: "error"` (`nodeId` + `message`).
3. Leave `enabled: false` until the user wants the schedule/hotkey live; then `automation_set_enabled`.
4. Prefer `trigger.schedule` + `action.agent` + `action.notify` for “every morning, do X and ping me”.
5. For “take this problem and run my workflow”, `automation_run` with `input`; do not rebuild the graph each time.
6. After save, tell the user the name, id, and to open the Automations page.

## Example 1 — daily agent briefing

```json
{
  "name": "Daily briefing",
  "enabled": false,
  "nodes": [
    { "id": "t", "type": "trigger.schedule", "data": { "label": "09:00", "schedule": { "kind": "daily", "hour": 9, "minute": 0, "intervalMinutes": 60 } } },
    { "id": "a", "type": "action.agent", "data": { "label": "Brief" } },
    { "id": "ctx", "type": "agent.context", "data": { "label": "Prompt", "agent": { "prompt": "Write a short morning briefing from {{output}}." } } },
    { "id": "rt", "type": "agent.runtime", "data": { "label": "Runtime", "agent": { "runtimeKind": "builtin" } } },
    { "id": "n", "type": "action.notify", "data": { "label": "Notify", "notify": { "body": "{{output}}" } } }
  ],
  "edges": [
    { "id": "e1", "source": "t", "target": "a" },
    { "id": "e2", "source": "a", "target": "n" },
    { "id": "er", "source": "rt", "target": "a", "sourceHandle": "slot", "targetHandle": "runtime" },
    { "id": "ec", "source": "ctx", "target": "a", "sourceHandle": "slot", "targetHandle": "context" }
  ]
}
```

## Example 2 — run with input

Existing graph: manual trigger → agent (prompt uses `{{output}}`) → notify.

```
automation_run id=<id> input="Draft a reply to this email: …"
```

The trigger text becomes that string; the agent prompt interpolates it.

## Example 3 — HTTP then branch

```json
{
  "name": "Health ping",
  "nodes": [
    { "id": "t", "type": "trigger.manual", "data": { "label": "Manual" } },
    { "id": "h", "type": "action.http", "data": { "label": "GET", "http": { "method": "GET", "url": "https://example.com/health", "headers": "", "body": "" } } },
    { "id": "i", "type": "logic.if", "data": { "label": "OK?", "if": { "op": "contains", "value": "ok" } } },
    { "id": "yes", "type": "action.notify", "data": { "label": "Up", "notify": { "body": "healthy: {{output}}" } } },
    { "id": "no", "type": "action.notify", "data": { "label": "Down", "notify": { "body": "unhealthy: {{output}}" } } }
  ],
  "edges": [
    { "id": "e1", "source": "t", "target": "h" },
    { "id": "e2", "source": "h", "target": "i" },
    { "id": "e3", "source": "i", "target": "yes", "sourceHandle": "true" },
    { "id": "e4", "source": "i", "target": "no", "sourceHandle": "false" }
  ]
}
```
