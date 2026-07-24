# Research: xAI grok 在 Responses API 上的托管搜索(web_search)wire 形态 & Kivio 引用丢失归因

- **Query**: grok-4.5 在 openai_responses 上托管 web search 的确切返回形态;Kivio 当前注入/解析是否导致搜索来源引用丢失
- **Scope**: mixed(本地 request_debug 证据 + 直连 loki curl 抓原始 JSON + xAI 官方文档)
- **Date**: 2026-07-24

## TL;DR 结论

- **注入形态是对的**:xAI 官方文档明确 `{"type":"web_search"}` 就是 OpenAI Responses API 上的正确托管搜索工具名。Kivio 在 `responses.rs:334` 注入的正是它。**不用改注入。**(排除假设 D)
- **网关没吞结果**:直连 loki(`https://newapi.loki.cc.cd/v1/responses`)裸调 grok-4.5,原始响应里 **`web_search_call` 项带完整 `action.sources[]`(13 个 URL),最终 `message` 带 `url_citation` 注解**。引用链路完整。(排除假设 B)
- **真正原因 = C + A**:
  - **C(模型行为)**:真实 Kivio 会话里 grok 发出 hosted `web_search_call`(query 被抓到)后,**在同一轮又发了一个客户端 `web_fetch` 函数调用**,该轮以 `finish_reason=tool_calls` 结束、**没有产出带 `url_citation` 注解的最终 message**,所以那一轮 citations 为空;grok 转而自己 fetch 网页凑答案。这是模型选择(grok 偏好客户端 fetch),被"同时暴露 web_fetch + hosted web_search"放大。GPT 在相同 21 工具环境下会直接用 hosted 结果并带 citations,grok 会岔到 fetch。
  - **A(解析缺口)**:grok 的 `web_search_call.action.sources[]`(它实际检索过的 URL 列表)**Kivio 完全没解析**——只取了 `action.query`。即便模型岔到 fetch、never 产出 `url_citation` 的最终 message,`action.sources` 里的来源本可以拿来填卡。

## Findings

### 本地证据 1:request_debug 记录(真实 Kivio 会话)

文件:`~/Library/Application Support/com.zmair.kivio/request_debug/records.jsonl`(20 条)。
`response` 是解析后摘要(非原始 JSON),`webSearch` 字段是 Kivio 解析出的 `{queries, citations}`。

同一个 "NVDA 收盘价" 问题的对照:

| 记录 | 模型 | url | 注入 tools | response.webSearch | finishReason | 该轮 toolCalls |
|---|---|---|---|---|---|---|
| REC 2 | grok-4.5 | loki `/v1/responses` | `[{type:web_search}]` + 20 fn | `{citations:[], queries:["NVDA...","..."]}` | tool_calls | 客户端 `web_fetch`(yahoo) |
| REC 7 | grok-4.5 | loki | web_search + fn | `{citations:[], queries:["北京 明天 天气预报"]}` | tool_calls | `web_fetch`(wttr.in) |
| REC 8 | grok-4.5 | loki | web_search + fn | `{citations:[], queries:[...]}` | tool_calls | `web_fetch`(cma.cn) |
| REC 17 | gpt-5.6-sol | xb1520 `/v1/responses` | web_search + fn | `{citations:[{title,url}×2], queries:[5 条]}` | (success) | — |

**关键**:grok 每次 hosted 搜索都拿到了 query,但 citations 恒为 `[]`,且该轮立即岔向客户端 `web_fetch`;GPT 在同样工具集下 citations 正常填充(URL 带 `utm_source=openai` 标记,证明来自 OpenAI 真实 hosted 后端)。

### 本地证据 2:直连 loki 裸调(原始 JSON,证明引用链路完整)

保存于同目录:
- `grok-curl-web_search.json`(仅 `[{type:web_search}]`,非流式)
- `grok-curl-web_search-plus-fn.json`(`web_search` + 一个客户端 `web_fetch` 函数工具,复现 Kivio 条件)

两次都 **HTTP 200 + `status:"completed"`**,`output[]` 结构:

```
reasoning → web_search_call(action.type="search", query, sources[16]) ×N
          → web_search_call(action.type="open_page", url) ×N
          → message(content[0].type="output_text", annotations=[url_citation ×3~4])
```

`web_search_call` 项的完整形态(grok 特有,比 OpenAI 更富):
```json
{
  "type": "web_search_call", "status": "completed",
  "action": {
    "type": "search",                       // 也可能是 "open_page"(带 url,无 query)
    "query": "NVDA stock price closing",
    "sources": [ {"type":"url","url":"https://finance.yahoo.com/quote/NVDA/"}, ... 16 条 ]
  }
}
```

最终 message 的注解(与 OpenAI 同构):
```json
{"type":"url_citation","url":"https://finance.yahoo.com/quote/NVDA/","start_index":47,"end_index":91,"title":"1"}
```
(注意 grok 的 `title` 只是序号 "1"/"2"/…,不是真实网页标题。)

**顶层无 `citations` 数组**(`"citations" in response == False`)。即在 `/v1/responses` 端点上,xAI 用 `url_citation` 注解返回引用,**不**用顶层 `citations` 数组。

**复现结论**:即便带上客户端 `web_fetch` 工具,单轮非流式调用里 grok 仍然全程用 hosted 搜索并产出 url_citation、并未岔去 fetch。空引用只发生在真实的**流式多工具 agent 轮**里,grok 在那一轮选择了客户端 `web_fetch`。属模型非确定性行为。

### 本地代码:Kivio 的注入与解析

`src-tauri/src/chat/model/responses.rs`:

- **注入**(`request_body`)`responses.rs:333-338`:`if request.options.builtin_web_search { tools_arr.push({"type":"web_search"}) }`,并设 `tool_choice:"auto"`。对所有 openai_responses provider 统一,无 per-provider 分叉。**注入正确。**
- **流式解析** `responses.rs:751-763`:`web_search_call` 事件里只读 `action.query` / `action.queries` 累加为查询词;**未读 `action.sources`**。
- **流式引用** `responses.rs:770-780`:`response.output_text.annotation.added` 里读 `annotation.type=="url_citation"` 的 `url`/`title`。
- **非流式解析** `web_search_from_responses_output` `responses.rs:991-1059`:`web_search_call` 只取 `action.query`(`:998-1009`);`message` 只取 `content[].annotations[type=url_citation]`(`:1011-1049`)。**同样忽略 `action.sources`,也不读顶层 `citations`。**

结论:Kivio 的 Responses 解析已正确处理 `url_citation`(与 GPT 共用,故 GPT 完美),但 **(1) 从不解析 grok 特有的 `web_search_call.action.sources[]`;(2) 从不读顶层 `citations` 数组**(该数组只在 xAI 原生 SDK / chat-completions 路径出现,见下)。

### 历史线索对照:`web_search.rs` 的 Lens `search_grok`

`src-tauri/src/web_search.rs:490-645`(Lens 专用,与 chat 的 responses.rs 是**两套独立解析**):
- 注释(`:486-489`)说旧版 chat completions 的 Live Search(`search_parameters`)2026-01 停用,改用 Responses API + `tools:[{type:web_search}]`。
- `parse_grok_response`(`:578-645`)做多路兜底:顶层 `citations` 数组 → `output[].content[].annotations[].url` → chat-completions `choices`。
- 它读**顶层 `citations`** 是对的——那是 **xAI 原生 SDK (`response.citations`) / 非 Responses 端点**的惯例;但在 `/v1/responses` 端点上顶层 citations 不出现(本次直连证实),真正生效的是 `url_citation` 注解分支。

### 外部文档(docs.x.ai,2026-07 现存)

来源:`https://docs.x.ai/docs/guides/live-search`(页面标题实为 **"Web Search"**)。

- **工具名对照表(文档原文)**:xAI SDK = `web_search`;**OpenAI Responses API = `web_search`**;Vercel AI SDK = `xai.tools.webSearch()`。"also supported in all Responses API compatible SDKs"。→ 证实 Kivio 注入名正确。
- **Web Search 参数**(可选,均非必填):`allowed_domains`(≤5,与 excluded 互斥)、`excluded_domains`(≤5)、`enable_image_understanding`、`enable_image_search`。**未见** `mode` / `max_search_results` / `sources`(这些是旧 chat-completions `search_parameters` 的字段,已不在 Responses web_search 工具上)。
- **引用返回方式**:xAI **原生 SDK** 暴露 `response.citations`(顶层);**AI SDK** 暴露 `sources`;**OpenAI Responses 路径**则是 message 的 `url_citation` 注解(本次直连原始 JSON 证实)。
- **另有独立托管工具**(文档左栏 Tools 分类):`Web Search` / **`X Search`(搜索 X/Twitter 平台,独立工具,非通用网页搜索)** / `Code Execution` / `Collections Search (RAG)` / `Remote MCP`。X Search 页面精确 slug 未定位到(试了几个 404),但导航证实其存在;它面向 X 帖子,**不**是我们要的通用 web 搜索,当前 `web_search` 就是对的通用工具。
- 旧 `search_parameters`(Live Search)已随 chat-completions 淡出,官方指引迁移到 Responses API + `web_search` 工具。

## 若要改 Kivio 的建议(file:line)

**证据证实的最小改动(补 A)**:把 grok `web_search_call.action.sources[]` 当作 citations 的兜底来源,这样即使模型岔去 web_fetch、never 产出 `url_citation` 的最终 message,搜索卡也能显示 grok 实际检索过的 URL。

- `src-tauri/src/chat/model/responses.rs:996-1010`(非流式 `web_search_from_responses_output` 的 `web_search_call` 分支):除现有 `action.query` 外,读 `action.sources[].url`(仅当 `type=="url"`),push 进 `result.citations`(title 缺省用 host 或 url)。注意去重与既有 `url_citation` 合并。
- `src-tauri/src/chat/model/responses.rs:751-763`(流式 `web_search_call` 的 `output_item.done` 分支):同样从 `action.sources` 抽 URL 调 `push_web_search_citation`。
- **可选**:在 `push_web_search_citation`/解析处保留 `url_citation` 优先级(真实带注解的引用应盖过 sources 兜底),避免把"搜过但没被引用"的 16 条 URL 全塞进卡里显得噪。可只在最终 citations 为空时才回退到 sources。

**推断性(非必须,属行为调优,未经证实)**:
- grok 岔向客户端 `web_fetch` 是模型行为。若想让 grok 更依赖 hosted 结果,可考虑在 `builtin_web_search` 开启时对 openai_responses+grok 收窄客户端 `web_fetch` 的暴露,或在系统提示里提示"已内置联网,优先用内置结果"。**这是设计取舍,本次无证据表明必须改**,且 GPT 不受影响,故不建议无差别改。
- 顶层 `citations` 数组解析在 chat 的 responses.rs 里可不补——`/v1/responses` 端点实测不返回它;`web_search.rs`(Lens)已单独兜底。

## Caveats / Not Found

- **文档证实**:`web_search` 是 Responses 正确工具名及其参数(allowed/excluded_domains、image 开关);`x_search`/`code_execution` 作为独立托管工具存在;Live Search/`search_parameters` 已淡出。
- **直连实测证实**(loki 网关,非 xAI 官方 endpoint):grok Responses 返回 `web_search_call.action.sources[]` + message `url_citation` 注解、无顶层 `citations`;带客户端工具的单轮调用不岔去 fetch。
- **推断**:真实会话空引用归因于"流式多工具轮里 grok 选择客户端 web_fetch → 该轮无 url_citation 最终 message"——基于 request_debug 摘要(finishReason=tool_calls + toolCalls=web_fetch + citations=[])推断,未抓到那几轮的**原始**流式 JSON(request_debug 只存解析后摘要)。可信度高但非 100% 原始证据。
- **未定位**:xAI `x_search` 工具页精确 URL(几个 slug 均 404);`action.sources` 字段未在 Web Search 文档正文明确记载(是实测观察到的 grok 特有字段,可能未文档化,未来可能变)。
- 直连用的是 loki 第三方网关的 grok-4.5,不是 `api.x.ai` 官方端点;官方端点行为未直接验证(但 loki 表现与官方文档一致)。

## 附:原始证据文件

- `research/grok-curl-web_search.json` — 仅 web_search 的裸调原始响应(120KB)
- `research/grok-curl-web_search-plus-fn.json` — web_search + 客户端 web_fetch 的裸调原始响应(复现 Kivio 条件,128KB)
- 均已确认不含 API key。
