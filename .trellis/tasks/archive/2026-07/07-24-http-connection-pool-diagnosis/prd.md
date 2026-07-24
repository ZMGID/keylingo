# PRD — HTTP 连接池陈旧连接修复 + 错误链诊断

## 背景 / 问题

用户反馈(Kivio macOS 2.8.2，OpenAI Chat 格式渠道)：进程连续运行数天后，
chat_tools_planning 阶段在**所有模型**上失败，UI 只显示 `error sending request`，
请求调试记录 `statusCode = null`；同一台 Mac、同一 Key/头/工具/上下文直接重放请求返回
HTTP 200；**重启 Kivio 即恢复**。指向 Kivio 自身长生命周期 reqwest/hyper 连接池里
的陈旧连接被复用。

## 已核实的代码事实（静态分析，非推测）

- **A. 错误链被全局丢弃**：错误只存 `format!("{} Error: {}", label, err)`
  (`api.rs:544`)，用 reqwest `Display`，不含 `.source()` 底层链（DNS/TCP/TLS/h2 reset/
  connection closed）。全仓 `grep .source()` = 0 处。→ 无法确认失败在哪一层。
- **B. HTTP 客户端零保活/零主动清理**：`build_http_client`(`api.rs:150`) 仅设
  `connect_timeout(20s)` + `read_timeout(300s)`；**无** `tcp_keepalive` /
  `http2_keep_alive_*` / 显式 `pool_idle_timeout`。client 为 `AppState.http` 单例，
  开机建、关机销毁，GUI chat / 外部 CLI / 子代理三处共用。
- **C. 重试分类窄**：`is_retryable_error = is_timeout() || is_connect()`
  (`api.rs:233`)，其它网络错（连接中途被关 = `is_request` 类）不重试。
- **D. 时间吻合**：退避 5s→10s，3 次尝试 ≈ 15s 退避 + 3 次瞬时失败 ≈ 15.1s，与报告一致；
  反推失败为「瞬时」（非 60s 总超时/20s 连接超时耗尽），且循环确实重试过。

## 无法从代码确定的

失败具体落在哪一层（陈旧 h2 连接 / TCP RST / TLS / 中转渠道抖动）**取决于被吞掉的
`.source()`**。故根因为「合理推测」，代码本身证明不了。

## 范围（本任务只做 A + B；C 缓做）

- **A（先做，零风险）**：错误落库/展示时展开完整 `.source()` 链，替换裸 Display。
  影响 `send_with_retry_status_policy` 的 Err 分支及 record_debug_failure 路径。
- **B（同时做，低风险）**：`build_http_client` 增加连接池老化控制：
  `tcp_keepalive`、`http2_keep_alive_interval` + `http2_keep_alive_while_idle`、
  显式 `pool_idle_timeout`。目标是让 hyper 主动探活并及时淘汰死连接。
- **不在本任务**：C（放宽重试分类）——等 A 拿到真实错误类型再针对性做，避免盲目扩大
  重试把非幂等请求重发。Grok anyOf(已由 `076f50d` 修复)、Claude 上游 500(非本地) 均不做。

## 验收标准

- [ ] 任一网络层错误在请求调试记录 `error` 字段中包含完整 source 链文本
      （能区分 DNS / connect / TLS / connection closed / h2 reset 等），而非仅
      `error sending request`。
- [ ] `build_http_client` 显式配置 tcp keepalive + http2 keepalive + pool idle timeout；
      有一个 Rust 单测断言构建的 Client 不 panic（builder 参数合法）。
- [ ] `cargo test`（api.rs 相关）+ `npm run typecheck` 全绿；三处共享 client 的调用面
      （chat / 外部 CLI / 子代理）不改契约。
- [ ] 实机冒烟：正常发一条带工具的消息成功；错误链改动不影响成功路径。
- [ ] 长期验证靠线上复现：下次再遇 `error sending request` 时调试记录应给出可定位的层级
      （本任务交付即视为满足「可诊断」，真实根因确认在后续观测）。

## Notes

- 这是「先让它能被诊断 + 顺手硬化最可能的一类」，不是「宣称已定位根因」。真正根因确认
  依赖 A 上线后的下一次观测。避免 fix-forget-repeat。
