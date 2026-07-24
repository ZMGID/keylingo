# Built-in (Provider-Native) Web Search

## Scenario: Session-scoped web-search mode + hosted search injection/parsing (task 07-23)

### 1. Scope / Trigger

- Trigger: any change to the chat web-search mode (off/builtin/third_party), the hosted-search
  tool injection in model adapters, the citation parsing, the realtime search card, or the
  per-family reasoning-effort mapping.
- Hosted web search is a **server-side tool**: the provider executes the search inside the model
  turn; the agent loop never sees a tool call. Everything the UI shows must be parsed from the
  response wire and synthesized into a display-only `ToolCallRecord`.

### 2. Signatures

- Rust enum: `chat::types::WebSearchMode { Off, Builtin, ThirdParty }` (serde snake_case);
  `Conversation.web_search_mode: Option<WebSearchMode>` (None = legacy, falls back to global).
- Resolution: `WebSearchMode::resolve(conv_mode, &settings)` — None → global
  `chat_tools.native_tools.web_search` (on ⇒ ThirdParty, off ⇒ Off). Byte-compatible for old
  conversations.
- Capability: `model_metadata::builtin_web_search_supported(&ModelProvider) -> bool` — true only
  for `ProviderApiFormat::{OpenAiResponses, Gemini, AnthropicMessages}`. Frontend twin:
  `builtinWebSearchSupported(apiFormat)` in `src/api/tauri.ts` (must stay in lockstep).
- Request flag: `GenerateOptions.builtin_web_search: bool` (serde default false). Set only when
  `AgentRunConfig::builtin_web_search_active()` (mode==Builtin && provider supported).
- Effort mapping (single source of truth): `model_metadata::reasoning_effort_wire(kind, level)
  -> Option<String>` — OpenAI/Gemini clamp xhigh/max→high; Anthropic passes all tiers; None/blank
  ⇒ omit the field. All four adapters call it; never inline a per-adapter mapping.
- Parsed result: `GenerateOutput.web_search: Option<BuiltinWebSearch { queries, citations }>`;
  `WebCitation { title, url }`.
- Realtime: `StreamPart::WebSearch { queries, citations }` (process-internal only, never
  serialized across a boundary) + `WebSearchCardTracker` in `agent/stream.rs` (mirrors
  `ToolCallDraftTracker`: stable `websearch_<uuid>` id, reserved order slot, dedup accumulate,
  Running→Success flip).
- Tauri command: `chat_update_conversation(.., web_search_mode: Option<String>)` — accepts
  `off|builtin|third_party`; blank/unknown clears to None (follow-global).
- Debug probe (debug builds): `chat_probe/request.json` accepts `webSearchMode` for headless
  verification of the builtin path.

### 3. Contracts

**Injection (per adapter, only when `builtin_web_search` is true, appended to tools):**

| Adapter | Wire tool object | Endpoint constraint |
|---|---|---|
| responses.rs | `{"type":"web_search"}` | Responses only; gpt-5 on Chat Completions 400s |
| gemini.rs | `{"google_search":{}}` (sibling of functionDeclarations entry) | native generateContent only |
| anthropic.rs | `{"type":"web_search_20250305","name":"web_search"}` | org must enable in Console |
| openai.rs (Chat) | never inject | unsupported |

**Reasoning coupling (Responses)**: gpt-5-family does NOT execute hosted web_search under
minimal reasoning. `resolve_thinking(None)` defaults to high, so effort is always non-minimal;
do not remove that default without revisiting hosted search.

**Citation parsing (Responses wire — verified against live traffic 2026-07):**
- Streaming: `web_search_call` items arrive ONLY via `response.output_item.done`
  (`item.action.query`/`queries`); they do NOT reappear in `response.completed.output`
  (which may contain only `message`). Citations arrive via
  `response.output_text.annotation.added` (`annotation.type=="url_citation"`, url+title).
  A parser that reads only `response.completed` silently misses everything.
- grok (xAI) extra: `web_search_call.action.sources[]` = URLs the search hit. Collect them;
  ONLY if the whole turn produced zero `url_citation` (grok often branches to client
  `web_fetch` and never writes an annotated message) promote sources → citations (dedup by
  url, empty title). When annotations exist they win and sources are dropped.
- `finish()` must emit one final `StreamPart::WebSearch` snapshot unconditionally: some
  gateways (loki/newapi) restate `web_search_call` inside `response.completed`; the merge there
  updates state without emitting, so without the final frame the realtime card misses those
  sources. The tracker dedups, so the extra frame is idempotent.
- Gemini: `candidates[].groundingMetadata` (`webSearchQueries` + `groundingChunks[].web`),
  usually only in the LAST stream chunk — realtime display is best-effort there.
- Anthropic: `server_tool_use`(name==web_search, query) + `web_search_tool_result` blocks,
  assumed complete at `content_block_start`.

**Card synthesis + placement:**
- Display-only `ToolCallRecord`: `name:"web_search"`, `source:"native"`,
  `structured_content: {type:"builtin_web_search", provider, queries, citations}` — the
  frontend routes on `structured_content.type`, so it never collides with the third-party
  `search_web` tool card (same name/source!). The record goes into `tool_records` + a Tool
  segment but NEVER into the model transcript (not replayable).
- Order: planning/synthesis reserve one order slot between the reasoning and text segments
  (`SegmentBuilder::reserve_order()`). Card `step_number` must stay None so the frontend
  compares purely by order → card renders above the answer. Unused slots leave harmless holes.
- Mutual exclusion: streaming path materializes the card via `WebSearchCardTracker.take_card()`;
  the non-stream path uses `emit_builtin_web_search_card(order: Option<u32>)`. The streaming
  branch must NOT also populate the legacy `*_web_search` variable, or you get a double card.
- Tool gating: ThirdParty exposes client `search_web`; Builtin/Off strip it
  (`apply_web_search_mode_tool_filter`, `commands/tooling.rs`). Builtin relies on body
  injection only.

### 4. Validation & Error Matrix

| Condition | Behavior |
|---|---|
| `web_search_mode` missing in stored JSON | deserializes None → follow global (legacy-safe) |
| Builtin selected but provider unsupported | frontend greys the option; backend degrades to no injection (no error) |
| Any citation-parsing failure / unknown wire shape | silently degrade to `web_search: None`; never block the answer |
| Hosted search executed but zero citations AND zero sources | card shows queries only (honest; model summarized without citing) |
| Turn cancelled mid-stream | realtime card not finalized/persisted (same as tool drafts) |
| Unknown `web_search_mode` string in update command | cleared to None, not an error |

### 5. Good/Base/Bad Cases

- Good: Builtin + Responses provider → tools contains `{"type":"web_search"}` + client tools;
  card appears Running during search, flips Success with queries+citations, ordered above the
  answer text; persisted across reload.
- Base: mode Off (or legacy conversation with global web_search off) → no injection, no
  `search_web`, zero behavioral delta vs pre-feature builds.
- Bad: injecting `web_search` on Chat Completions (400), parsing citations only from
  `response.completed` (misses everything on stream), reusing the third-party card renderer by
  name instead of `structured_content.type`, or pushing the synthesized record into the model
  transcript.

### 6. Tests Required

- `model_metadata`: `reasoning_effort_wire_maps_per_family`, `builtin_web_search_supported_by_api_format` (alias coverage).
- Adapters: injection on/off body shape per family; `web_search_parsed_from_responses_output`
  (+ sources-fallback pair: sources-only promotes, annotation wins);
  `sse_stream_captures_builtin_web_search_from_events` (proves capture comes from stream events,
  completed carries only `message`); `sse_stream_sources_fallback_when_no_citation_arrives`;
  gemini grounding + merge dedup; anthropic SSE event pair.
- `chat/types.rs`: legacy Conversation without the field deserializes; `WebSearchMode::resolve` fallback table.
- `tooling.rs`: Off/Builtin strip `search_web`, ThirdParty keeps it, unrelated tools survive.
- stream.rs tracker: Running card order < text order; same-id incremental merge (segment emitted
  once); finalize Success; no card without frames.
- loop_tests: streaming single card above answer (FinalAnswer path must still append it);
  non-stream card uses reserved slot; no double card.

### 7. Wrong vs Correct

#### Wrong (silently loses every citation on streaming)
```rust
// Parse hosted-search traces only when the response completes:
"response.completed" => {
    if let Some(output) = response.get("output") { state.web_search = parse(output); }
}
```

#### Correct (accumulate per event; completed only merges; finish syncs the card)
```rust
"response.output_item.done"            /* web_search_call */ => state.push_query(..),
"response.output_text.annotation.added" /* url_citation */  => state.push_citation(..),
"response.completed"                    => merge_without_emit(..),
// finish(): promote grok action.sources[] if no url_citation arrived,
// then emit one final StreamPart::WebSearch snapshot (tracker dedups).
```

### Design Decisions

- **Session-scoped mode with remembered default**: mode persists per conversation
  (`web_search_mode`), while the frontend also remembers the last explicit pick in
  localStorage (`kivio.chat.lastWebSearchMode`) and applies it to new conversations at first
  send — the backend Builtin injection reads the conversation field, so the frontend must
  materialize the remembered default onto the conversation, not just display it.
- **`api_format` gates capability, not `ModelCapabilities.web_search`**: the model-DB
  "网络搜索" toggle is a metadata label only. Hosted search availability is a protocol
  property; switching a provider (e.g. a newapi gateway) to `openai_responses` is what
  lights up Builtin.
- **grok behavior is not a bug**: grok routinely does hosted search → then client `web_fetch`
  to verify primary pages, producing no annotations that turn. The sources fallback exists for
  exactly this; do not "fix" the fetching.
