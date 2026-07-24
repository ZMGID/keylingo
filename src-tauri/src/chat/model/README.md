# Kivio Chat Runtime Model Contract

The Chat runtime owns orchestration: conversation state, prompt construction, tool loop,
cancellation, persistence, and Tauri events.

Provider adapters own provider JSON and wire protocols. Runtime code should pass
`GenerateRequest` to a `LanguageModelProvider` and consume `GenerateOutput` plus
`StreamPart` events. OpenAI-compatible and Anthropic Messages are peer providers.

Rules:

- Runtime and tool loop code should not inspect OpenAI `choices`, Anthropic `content`
  blocks, SSE event names, or provider-specific headers.
- Provider adapters may use `serde_json::Value` freely at their wire boundary.
- Storage can keep legacy `api_messages` for compatibility, but new replay logic should
  prefer canonical model messages when available.
- Tauri event payloads remain stable: `chat-stream`, `chat-tool`, and `chat-context`
  are UI contracts, not provider contracts.
- Model-generated images are protocol-agnostic on the contract: adapters parse each
  wire format's output-image shape (Gemini `inlineData`, …) into `GenerateOutput.images`
  (and stream `StreamPart::ImageData`); runtime never inspects provider image JSON.
