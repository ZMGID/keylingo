---
name: kivio-diagnosing-mcp
description: 为 Kivio 添加、修改、测试 MCP server，排查连不上、认证失败、找不到工具或安装后工具没出现；区分独立 MCP、插件所属 MCP、连接器和外部 CLI 原生 MCP。
---

# Kivio MCP 配置与排障

依据 Kivio 2.9.6（2026-09-06）。先确认目标是 Kivio 的 MCP，还是当前外部 CLI 的 MCP。独立 MCP 保存在 `<app_data>/settings.json` 的 `settings.chatTools.servers`。不要把 Claude/Codex 的 server map 原样塞入该数组，也不写不存在的顶层 `mcpServers`。

先用 `kivio_inspect {"topic":"mcp"}` 读脱敏摘要。`managed:true` 的 server 应回到插件或连接器管理，不能为了修复它新建第二份独立 server。

## 添加/更新

将一个 server 配置写成 UTF-8 JSON 临时文件，使用实际已验证的程序、脚本绝对路径或 endpoint：

```json
{"name":"local-helper","transport":"stdio","command":"node","args":["C:/actual/path/server.cjs"],"env":{},"enabled":true}
```

```json
{"name":"remote-helper","transport":"streamable_http","url":"https://your-service.example/mcp","headers":{},"enabled":true}
```

示例路径/域名是占位，不能直接发起连接。字段：`name`、`enabled`、`transport`、`command`、`args`、`url`、`env`、`headers`、`cwd`、`enabledTools`。`cwd` 是已存在的绝对目录；stdio 的 command 是一个可执行文件，args 是参数数组，不是一整段 shell 命令。

调用 `kivio_configure {"action":"mcp_upsert","config_path":"<JSON文件路径>"}` 新增。更新要传实际 `id`，文件可只含要修改的字段，未指定字段包括原密钥都会保留；`env` / `headers` 若指定则**整个 map 替换**，编辑前保留仍需的键。不传 id 时同名会拒绝，避免重复安装。

服务配置缺省停用；要装好可用时写 `enabled:true`，再 `{"action":"mcp_test","id":"<返回ID>"}` 验证连接和工具名。测试可能启动本地 server 或连接网络，不是静态 JSON 校验；单纯查看配置用 inspect。响应成功只证明列出了工具，下一轮再做符合用户用途的最小调用。

移除用 `mcp_remove` 和实际 id。停用用 mcp_upsert 补丁 `{"enabled":false}`。两者都会清理原持久连接；不在运行中直接改 settings 文件。工具缺失时走扩展 → MCP 的编辑与测试入口。

## 诊断顺序

1. **身份/开关**：是否独立 server、enabled 是否 true；`enabledTools:[]` 表示不过滤工具，不是全部禁用。助手 MCP 白名单、Kivio Chat 只读筛选和规划模式还会进一步过滤。
2. **启动**：stdio 可执行文件是否真存在/在宿主 PATH，参数是否独立，cwd 是否正确。终端能运行不保证 GUI 进程 PATH 相同。不要把 Git Bash 语法塞进直接启动的 command/args。
3. **协议**：stdio stdout 必须是 MCP 消息；安装进度/日志发 stderr。HTTP 使用 `streamable_http`，不是 settings 中的 `http` 或旧 SSE 模式。插件 `.mcp.json` 可用 `type:"http"`，由插件加载器转换，别混淆两种 schema。
4. **认证**：只检查密钥是否存在和 header/env 名称。401/403 先看认证；TLS、404、DNS 和启动失败分别处理，不能全部归因于模型不支持。URL 不内嵌用户名密码。OAuth 连接器通过连接器授权页续期。
5. **发现/调用**：用 MCP 测试取得实际工具名；原始 server 工具名、Kivio 内部 ID 和传给模型的规范化名称可能不同，调用用当前工具 schema。新注册不刷新当前轮冻结的工具列表，下一轮复查。

不要完整打印 settings、env、headers、auth 或未脱敏 stderr；配置里的认证值是秘密，不是报告内容。缺依赖先说明具体依赖，不靠延长超时掩盖启动失败。

源码：`src-tauri/src/settings.rs::ChatMcpServer`；`mcp/{registry,manager,conn}.rs`；`self_config/mod.rs`；`chat/commands/tooling.rs`。
