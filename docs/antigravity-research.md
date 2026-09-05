# Antigravity OAuth 接入调研

调研日期：2026-09-05。结论来自项目文档和源码检查，未使用真实 Google 账号登录，也未验证在线模型调用。后续已按下述原生移植路线实现，使用方式与当前限制见 [OAuth 实现文档](provider-oauth.md)。

## 找到的实现

| 项目 | 检查版本 | 能力与适用方式 |
| --- | --- | --- |
| [Rahularya01/pi-antigravity](https://github.com/Rahularya01/pi-antigravity) | `70e8f6e3603c4926e29d31c97be5f9719003f84f`，包版本 0.7.1 | Pi 原生供应商扩展，Google OAuth、刷新、动态模型目录、流式响应与工具调用；最适合作为 Kivio 原生接入的协议参考。 |
| [cortexkit/antigravity-auth](https://github.com/cortexkit/antigravity-auth) | `626cce39848b4b6db2d64ff7cbed1f7b4dc0259d`，包版本 2.2.1 | 共享核心加 Pi/OpenCode 插件，Pi 包名为 `@cortexkit/pi-antigravity-auth`；可交叉核对端点和请求转换。 |
| [router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) | `5208aec703b5ce7e3445f6e9d91cc13b3e78003a` | 即用户提到的 CPA 类 2API 项目，包含 Antigravity 登录与兼容 API 代理；适合独立代理服务路线。 |

三份检查版本均附 MIT 许可证；移植代码应保留相应版权和许可文本。源码开源不等于 Google 官方支持这些集成，也不保证账号权限或接口持续可用。

Pi 第一份扩展的使用入口是 `pi install npm:pi-antigravity`，随后 `/login antigravity`；CortexKit 的 Pi 登录名是 `google-antigravity`。这些是上游使用方式，并未在用户电脑安装或执行。

## 关键协议

- 浏览器 Authorization Code + PKCE，与已有 Codex/Kimi 的设备码流程不同。Rahul 的实现生成彼此独立的随机 state 与 verifier，在 `http://localhost:51121/oauth-callback` 接收回调并校验 state；取消或完成后清理监听器。
- Google token endpoint 用于授权码交换和 refresh token 刷新；刷新时保留服务端未轮换的旧 refresh token。Kivio 应继续使用系统凭据库，而不是复制上游的 JSON 凭据文件方案。
- 登录后通过 `loadCodeAssist` 获取账号的 project，再通过 `fetchAvailableModels` 拉取模型目录。CPA 在缺失 project 时明确报错；Kivio 应优先采用此行为，不借用他人的 project，也不把生成的备用 ID 当作已验证项目。
- 推理使用 Cloud Code Assist 的 `v1internal:streamGenerateContent?alt=sse`。请求是带 `project`、`model`、`request` 等字段的专用封装，并非普通 OpenAI 请求；流式消息还需要展开 `response`。
- 模型目录可能包含 Gemini、Claude、GPT-OSS，实际权限以账号返回为准。目录中 `models` 对象的键是运行时模型 ID，内部 `MODEL_PLACEHOLDER_*` 字段不可直接用于推理。
- 工具 schema 不能统一照搬：检查的 Pi 实现对 Gemini 使用 `parametersJsonSchema`，对 Claude/GPT-OSS 使用经过裁剪的 `parameters`。还需处理思考参数、工具结果和 thought signature。

源码定位：

- [Pi OAuth](https://github.com/Rahularya01/pi-antigravity/blob/70e8f6e3603c4926e29d31c97be5f9719003f84f/src/auth/oauth.ts)
- [Pi 项目与模型发现](https://github.com/Rahularya01/pi-antigravity/blob/70e8f6e3603c4926e29d31c97be5f9719003f84f/src/client/client.ts)
- [Pi 请求与流式适配](https://github.com/Rahularya01/pi-antigravity/blob/70e8f6e3603c4926e29d31c97be5f9719003f84f/src/stream/stream.ts)
- [CPA 登录](https://github.com/router-for-me/CLIProxyAPI/blob/5208aec703b5ce7e3445f6e9d91cc13b3e78003a/sdk/auth/antigravity.go)

## Kivio 接入建议

优先移植 Pi 的登录与请求协议到 Rust，复用 Kivio 现有的凭据存储、供应商设置和 Gemini 消息处理基础。Pi 扩展依赖其 Node 运行时和扩展接口，不能直接作为当前 Tauri Rust 后端的即插即用依赖。

预期体验：添加 Antigravity OAuth → 浏览器 Google 登录 → 自动接收回调 → 获取账号可用模型 → 选择并调用。前端不展示设备码，而显示正在等待浏览器授权；支持取消、超时与重新登录。

实现涉及 `provider_oauth.rs` 的浏览器会话与刷新、Antigravity 专用推理适配、模型发现、前端 OAuth 类型和面板。至少验证 state 拒绝、回调路径、取消释放端口、令牌刷新、项目缺失、模型 ID 解析，以及文本/流式/多轮工具调用。真实登录和模型调用仍需账号持有人完成浏览器授权。

CPA 是另一条可行路线：由 CPA 登录和适配，Kivio 使用现有兼容 API 供应商访问本地代理。该路线可以少改推理代码，但需要额外管理代理进程、端口和配置。对于在 Kivio 内直接登录的目标，原生移植更贴近需求。
