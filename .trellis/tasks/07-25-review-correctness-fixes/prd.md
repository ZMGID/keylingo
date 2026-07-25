# PRD — 修复审查发现的正确性 Bug

## 背景

全项目审查(5 个子系统并行 code review)后,安全项(敏感目录 guard 缺失 / himalaya 明文密码 / iframe sandbox)由用户判定暂缓。本任务收拢**用户判定重要的 5 个正确性 Bug**,逐条修复。死代码删除已在 `ecc8366` 单独提交,不在本任务范围。

每条 Bug 相互独立、可单独验证。无强制先后顺序;按下方编号推进即可。

## 核实状态（2026-07-25 逐条读代码验证）

| 编号 | 结论 | 关键证据 |
|---|---|---|
| B1 | ✅ 确认真 bug(非「疑似」) | `liveGroup` 是 store 同一对象引用,memo deps 流式期全部引用不变 → 冻结 |
| B2 | ✅ 确认 | `retry_delay_ms:298-299` retry_after 分支无 `.min(cap)`,指数退避分支才有 |
| B3 | ✅ 确认 | `BackgroundCommand`(`shell.rs:582-595`)无 `conversation_id` 字段 |
| B4a | ✅ 确认 | rerank 设 `c.rerank_score`,但 `:346` 只写 `fused_score`,丢弃 rerank 分 |
| B4b | ✅ 确认 | `passes_threshold:418` 对 rerank_score=None 的 tail 返回 false → 静默截到 rerank_top_k |
| B4c | ⚠️ **非 bug,撤下** | `:228-243` 全局 500 上限是带注释的刻意设计,仅字段文档措辞需修 |
| B5 | ✅ 确认(但严重度低于表述) | `should_retry_sub_agent:117` 谓词 `err != "cancelled"` 对任何非取消错误都重试 |

## 范围内的 Bug

### B1 — 多模型并答:流式列 memo 冻结（✅ 已确认真 bug）
- **现象**:多模型同时回答时,流式栏卡在首帧空内容,直到全部落库才一次性显示。
- **证据**:`src/chat/MessageGroup.tsx:287-295`(配合 `:275`)。`columns` 的 `useMemo` deps 为 `[live, liveGroup, messages]`,但 store 原地 mutate 列内容(`groupStreamingStore.ts:125-151` 返回稳定引用),`liveGroup` 引用永不变 → memo 不重算,`streamColumnToMessage(col)` 冻结在首帧。`useGroupsVersion()`(:275)虽触发重渲但 deps 引用相等。
- **方向**:把 version 纳入 memo deps —— `const v = useGroupsVersion(); useMemo(..., [live, liveGroup, messages, v])`。
- **验收**:
  - [ ] 先手动复现:两个模型并答,确认流式栏是否真冻结(证实/证伪「疑似」)。
  - [ ] 修后并答流式栏实时滚字。
  - [ ] 单模型对话无回归。

### B2 — Retry-After 无上限（可导致 24h 挂起）
- **现象**:被限流时若服务器/中转返回超大 `Retry-After`(如 86400),该次重试会真睡这么久(仅靠 250ms 取消轮询能打断)。
- **证据**:`src-tauri/src/api.rs:231-236`(`parse_retry_after` 只解析秒数,忽略 HTTP-date)+ `:297-303`(`retry_delay_ms` 对 retry_after 秒数 `saturating_mul(1000)` 无封顶)。
- **方向**:对 retry_after 结果 `.min(RETRY_MAX_DELAY_MS)` 或设合理上限(如 60s)。HTTP-date 形式可选支持,不支持时应退回指数退避(而非静默睡 0)。
- **验收**:
  - [ ] 单测:`Retry-After: 86400` → 实际等待被 clamp 到上限。
  - [ ] 单测:非法/date 形式 `Retry-After` → 退回指数退避,不 panic。

### B3 — 后台作业不按会话隔离（跨会话可见/误杀）
- **现象**:AI 起的后台命令存在全局清单,A 会话能列出并 `kill` B 会话的进程;注释却写「本会话」。
- **证据**:`BackgroundCommand`(`shell.rs:584`)无 `conversation_id`;`state.background_commands` 单一全局 map;`bash_output`(无 job_id 列全部)/`kill_background` 接受任意 job_id;`mcp/native_registry.rs:790` 注释宣称「本会话」。
- **方向**:给 `BackgroundCommand` 加 `conversation_id`,`bash_output`(列表)/`kill_background` 按当前会话过滤。若判定隔离成本过高,退而修正注释别宣称「本会话」——**二选一,不留名不副实的注释**。
- **验收**:
  - [ ] 决策记录在 implement.md:做隔离 or 仅改注释。
  - [ ] 若隔离:单测/手测确认 A 会话看不到 B 会话作业。
  - [ ] app 退出清扫(`kill_all_background_commands`)仍杀全部,不受会话过滤影响。

### B4 — 知识库检索三处静默坑
- **B4a rerank 后 hits 仍写 fused 分**:`retrieval.rs:346` —— 返回的 score 与实际 rerank 排序不单调。方向:rerank 命中时用 rerank 分填 `ScoredChunk.score`(回退 fused)。
- **B4b 阈值把结果静默截到 `rerank_top_k`**:`retrieval.rs:410-418`+`272-290` —— 当 `rerank_top_k < context_top_k` 且 `min_score>0`,只有前 rerank_top_k 个被打分、其余判死,最终最多返回 rerank_top_k 条。方向:送 rerank 数取 `max(rerank_top_k, context_top_k)`,或 tail 用 fused 分兜底而非直接判死。
- **B4c 候选池全局上限**:~~伤多库召回~~ —— **核实后撤下**。`retrieval.rs:228-243` 的全局 500 上限是带注释的刻意设计(「keeps many mounted libraries from linearly blowing up rerank cost / context size」),不是 correctness bug。**唯一遗留**:`candidate_k` 字段/注释若写成「per-library」需改措辞对齐「全局池」。是否调高上限属调优,不在本任务。
- **验收**:
  - [ ] B4a:构造 rerank 场景,断言返回 hits 的 score 随排名单调不增。
  - [ ] B4b:`rerank_top_k=5, context_top_k=20, min_score>0` → 返回条数不被静默截到 5。
  - [ ] `kb_retrieval_test` 诊断路径同步生效(与生产共用 `retrieve`)。

### B5 — 子任务出错重放副作用（✅ 已确认，注意实际触发窗口较窄）
- **现象**:`agent` 子任务报错会**整个重跑一遍**(含已发生的写文件),本意只想在「空响应」时重试。
- **证据**:`sub_agent.rs:112-120` `should_retry_sub_agent` 谓词为 `matches!(outcome, Err(err) if err != "cancelled")` —— 对任何非取消 `Err` 都重试,没窄化到空响应。重试从头跑 `run_agent_loop`(fresh system+user 消息)。
- **nuance**:`run_agent_loop` 只在**致命错误**(provider 失败等)时返回 `Err`;单个工具失败是喂回模型的、不会让整个 loop 报错。故「重放写文件」仅发生在「已跑过若干轮(某轮已写文件)后 loop 才致命报错」这种较窄窗口。真但没表述的那么频繁。
- **方向**:收窄重试谓词到「空响应」那一类错误(复用 `:1287` 对空响应错误串的判定),其余 `Err` 直接冒泡不重试。
- **验收**:
  - [ ] 单测:非空响应的致命 `Err` → 不重试(不重复副作用)。
  - [ ] 单测:空响应 → 仍重试一次。

## 非目标
- 三个安全项(敏感目录 guard / himalaya 0600 / iframe sandbox)——用户暂缓,不在本任务。
- 死代码删除——已在 `ecc8366` 完成。
- KB dedup O(n²) 优化、compaction.rs 拆分、契约侵蚀收敛、内联按钮整改等「低/维护」优先级项——不在本任务。

## 验收总纲
- [ ] `cargo check --lib` 零新增 warning;相关子系统测试全绿(baseline: KB 9/9、external_agents 128/0)。
- [ ] 前端 `npm run lint` + `npm run typecheck` 通过(B1 涉及)。
- [ ] 每个 Bug 附最小可运行 check(单测或复现记录)。
