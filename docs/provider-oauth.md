# Model provider account authorization

## Research and scope

The installed Hermes Agent source was inspected at `%LOCALAPPDATA%/hermes/hermes-agent`, commit `63279301bcbdc185c1b07b98a9312eb0c862f26d` (2026-09-05 inspection). Its Python `hermes_cli/auth.py` implements Codex device authorization, token refresh, and runtime credential resolution. `agent/codex_headers.py` supplies the account claim and client identity headers. Kivio ports these protocol details into Rust; users do not need Hermes or Python installed.

Sources:

- [Hermes authentication implementation](https://github.com/NousResearch/hermes-agent/blob/63279301bcbdc185c1b07b98a9312eb0c862f26d/hermes_cli/auth.py)
- [Hermes Codex headers](https://github.com/NousResearch/hermes-agent/blob/63279301bcbdc185c1b07b98a9312eb0c862f26d/agent/codex_headers.py)
- [OpenAI authentication documentation](https://developers.openai.com/codex/auth)
- [Kimi CLI device authorization](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/auth/oauth.py), also inspected in the locally installed Kimi CLI package
- [Hermes removal of Google Gemini CLI and Antigravity OAuth](https://github.com/NousResearch/hermes-agent/commit/7130d60861)

The inspected Hermes version configures Kimi through API keys rather than the Kimi Code device flow. Kimi OAuth therefore follows Kimi CLI's implementation. Antigravity is implemented from the Pi extension's protocol, cross-checked against CLIProxyAPI; it does not require either application to be installed. See [Antigravity research](antigravity-research.md).

## User flow

1. Settings → Model providers → Add → **Codex OAuth**, **Kimi OAuth** or **Antigravity OAuth**. Existing providers also offer an Authentication selector.
2. Click **Sign in**. Codex/Kimi display a device code; Antigravity opens Google sign-in and receives the browser callback automatically.
3. Open **Models**, fetch the model list, and enable the desired models.
4. Test a selected model. Settings save automatically. The same provider can be selected for chat and vision/translation calls.

Cancel stops this login. Closing the panel also cancels pending authorization. Disconnect removes Kivio's local authorization immediately; the settings update is automatically saved. It does not revoke other applications' sessions. Reauthentication currently uses Disconnect followed by Sign in.

Account eligibility and quota are controlled by the upstream service; an OAuth login does not itself guarantee model access. These providers support text generation, tools and vision input through Kivio's existing model layer. The separate image-generation API is not enabled for these account sessions.

## Implementation

- `src-tauri/src/provider_oauth.rs`: bounded device sessions, server-directed polling intervals, expiry/cancellation, credential storage, refresh and model discovery.
- `ProviderRequestConfig.oauth`: `{ provider, credentialId? }`. This contains metadata only. API keys retain their existing storage and behavior; a selected OAuth mode never falls back silently to an old API key.
- Secrets use native Windows Credential Manager / macOS Keychain through `keyring`. Versioned 2000-byte chunks support long tokens within Windows' credential size limit. The manifest switches after all new chunks have been written; previous chunks are then removed. No plaintext-token fallback exists.
- Concurrent refresh and disconnect operations are serialized. Refresh tokens remain in Kivio's credential store; the Hermes, Codex and Kimi credential files are neither imported nor modified.
- Credential resolution validates the exact provider endpoint and protocol before retrieving tokens. OAuth HTTP clients do not follow redirects. Errors do not include token endpoint response bodies or credentials.
- All normal model calls use the common model dispatcher. It resolves an ephemeral provider clone immediately before inference; refreshed access tokens are never written into settings or returned to the webview.
- Codex uses its account endpoint and Responses streaming (including for nonstreaming callers), sends `store:false` and `instructions`, and strips unsupported generation fields. Kivio identifies itself as Kivio and derives the account header from the access token. Existing Responses tools and reasoning parsing are reused.
- Kimi uses its device authorization endpoint and the OpenAI-compatible coding endpoint, with Kimi client/device headers. Model lists are fetched from the authenticated provider rather than hard-coded.
- Antigravity uses Authorization Code + PKCE, an independent random state, and a temporary listener bound to `127.0.0.1:51121`. The browser redirect is `http://localhost:51121/oauth-callback`. Cancellation, a five-minute timeout, and completion release the listener; an occupied port produces a retryable error. Callback errors never echo codes or query strings.
- Antigravity project metadata is discovered with `loadCodeAssist` and kept with the credentials in the native store. Missing projects require account setup in Antigravity followed by a new login. No project IDs are borrowed or synthesized. Refresh preserves the project and handles refresh-token rotation.
- Antigravity lists runtime IDs from `fetchAvailableModels`, preserving account-specific reasoning variants. Requests use its native Cloud Code Assist envelope; the Gemini adapter unwraps SSE responses and preserves tool IDs/signatures for Claude and GPT-OSS. Nonstreaming consumers collect the same stream, and incomplete streams report an error.
- Antigravity uses the daily Cloud Code Assist endpoint. Automatic endpoint/model substitution, remote callback pasting and built-in Google Search are not implemented. Normal agent search tools remain available. This is an unofficial integration; real account access and upstream compatibility still need live verification.
- Model discovery and connection tests accept unsaved authentication metadata, matching the rest of the settings workflow.

## Attribution

Protocol logic in `provider_oauth.rs` is adapted from Hermes Agent (Copyright © 2025 Nous Research, MIT) and Kimi CLI (Moonshot AI, Apache-2.0), rewritten for Tauri/Rust with Kivio-owned storage and UI. License texts are retained in [licenses/hermes-agent-MIT.txt](licenses/hermes-agent-MIT.txt) and [licenses/kimi-cli-Apache-2.0.txt](licenses/kimi-cli-Apache-2.0.txt).

`provider_oauth/antigravity.rs` and the Antigravity Gemini bridge adapt protocol details from Rahul Arya's pi-antigravity at commit `70e8f6e3603c4926e29d31c97be5f9719003f84f`. Its MIT license is retained in [licenses/pi-antigravity-MIT.txt](licenses/pi-antigravity-MIT.txt).

## Authorized account identity

All three OAuth panels show the current account below the authorization status, including existing credentials. Antigravity reads email, name and subject from Google's fixed OpenID userinfo endpoint. Codex reads profile email and account ID from token claims; Kimi uses user ID (preferring `user_id` over `sub`), with email/name when available. Missing identity fields remain unavailable. JWT decoding is display metadata only, never an authorization decision. Only these identity fields reach the UI; tokens remain in native credential storage. Fetch failures can be retried independently of usage, and stale results are discarded when the account changes.

## Verification

Frontend tests exercise successful authorization, delayed polling, expiry errors, cancellation before the start response, and credential cleanup after unmount. Rust tests cover endpoint binding, required Codex body fields, account headers, refresh-token rotation parsing and both model-list shapes. An explicitly ignored native-store test writes only temporary synthetic credentials, verifies multi-chunk storage/rotation, then deletes them.

Run frontend tests with `npx vitest run src/settings/ProviderOAuthPanel.test.tsx src/settings/tabs/ProvidersTab.test.tsx src/settings/providerRequest.test.ts src/onboarding/validation.test.ts`. On Windows run Rust tests with `./scripts/win-cargo-test.ps1 --lib provider_oauth`. The native credential-store check can be run explicitly with `./scripts/win-cargo-test.ps1 --lib native_credential_store_roundtrip_and_rotation -- --ignored`.

Real account authorization and live model access require the account holder to complete the browser flow; mocked tests do not establish subscription access or service availability.

Antigravity verification on 2026-09-05: 57 focused frontend tests and 44 Rust tests (10 OAuth, 25 Gemini, 9 request-header tests) passed. The callback tests use real loopback sockets with synthetic state and denial/cancellation; no Google credentials are exchanged. Adapter fixtures cover the request envelope, endpoint binding, bearer authentication, response unwrapping, tool IDs and thought signatures. TypeScript, changed-file ESLint, production UI build and protocol export check passed. Real Google authorization and live Antigravity inference have not been performed.

Verification on 2026-09-05: 56 focused frontend tests passed; 5 OAuth unit tests, 9 request-header tests and 37 Responses-adapter tests passed. The native Windows credential-store test was also explicitly executed and passed, including cleanup. TypeScript, changed-file ESLint, and the production frontend build passed. A browser preview exercised the device-code panel with mocked authorization. Unauthenticated requests to both providers' device-authorization endpoints returned HTTP 200 with the required device fields; no account login or live inference was performed.

## OAuth account usage card

Settings shows an account usage card above Configuration for signed-in Kimi and Codex providers. Opening the detail page fetches once; Refresh fetches again. There is no background polling or reset-credit redemption. Antigravity usage is supported through dedicated remote quota endpoints.

The backend resolves and refreshes Kivio-owned credentials through the existing OAuth flow, validates the original provider endpoint, then makes a bounded GET with redirects disabled. Only normalized quota fields reach the webview. Transport errors never include tokens or raw response bodies. Missing usage percentages remain unavailable rather than appearing as 0% used.

Protocol references inspected locally on 2026-09-05:

- [Kimi CLI usage implementation](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/ui/shell/usage.py): `GET https://api.kimi.com/coding/v1/usages`, weekly `usage`, and `limits[].detail` / `limits[].window`. Supports used or remaining counters and absolute/relative reset times.
- [Hermes account usage](https://github.com/NousResearch/hermes-agent/blob/63279301bcbdc185c1b07b98a9312eb0c862f26d/agent/account_usage.py): `GET https://chatgpt.com/backend-api/wham/usage`, bearer authorization plus account header, primary/secondary windows and plan type. Kivio also displays code-review windows when returned.

The existing Hermes MIT and Kimi CLI Apache-2.0 notices apply to these protocol adaptations. The card displays remaining percentage; these are shared account limits, not Kivio-local token statistics. Tests cover remaining-to-used conversion, missing fields, numeric strings, reset times, manual refresh, failed requests and stale results after disconnect. Verification: 33 frontend tests, three Rust parser tests, TypeScript and changed-file ESLint passed. Live quota responses from an authenticated account have not been verified.

### Antigravity quota follow-up

The same card now supports authorized Antigravity accounts. It calls project-scoped retrieveUserQuotaSummary first, then retrieveUserQuota when grouped data is unavailable. Both use the existing fixed Cloud Code endpoint, account-bound native credentials, automatic refresh and redirect-disabled client. It parses grouped quota windows (including nested remaining fields) and model buckets, retains reset timestamps or upstream reset descriptions, and preserves missing/invalid fractions as unknown. It deliberately does not interpret fetchAvailableModels availability as real remaining quota. Remote OAuth may expose fewer windows than the Antigravity app. No local app credentials are imported, and no additional software is launched.

Protocol references: [CodexBar Antigravity notes](https://github.com/steipete/CodexBar/blob/main/docs/antigravity.md), [Antigravity-Manager quota schema](https://github.com/lbjlaq/Antigravity-Manager/blob/main/src-tauri/src/modules/quota.rs), and [Google Gemini CLI quota response example](https://github.com/google-gemini/gemini-cli/issues/14883). Independently implemented schema parsing; no upstream implementation is copied. Live OAuth quota access remains account-dependent and has not been verified in this change.
