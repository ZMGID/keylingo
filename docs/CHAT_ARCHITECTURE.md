# Kivio AI 客户端架构设计文档

## 概述

本文档记录了 Kivio 从"翻译+截图工具"向"完整 AI 桌面客户端"演进的架构设计决策。

## 设计原则

1. **独立开发，逐步融合**：先独立建设 AI 客户端，不破坏现有翻译器和 Lens 功能，后期再整合
2. **成熟的桌面体验**：UI/UX 采用成熟的桌面对话应用交互范式
3. **模块化架构**：前后端分离，组件化开发
4. **性能优先**：分页加载、流式响应、按需渲染

---

## 核心决策记录

### 1. 数据存储方案

**决策**：每条对话一个独立 JSON 文件 + 索引文件

**原因**：
- 隔离性好，单个对话损坏不影响其他
- 便于备份和迁移
- 文件系统天然支持

**方案对比**：
| 方案 | 优点 | 缺点 | 选择 |
|------|------|------|------|
| 单个 JSON | 简单 | 大文件性能差 | ❌ |
| SQLite | 查询强大 | 架构复杂 | ❌ |
| **独立文件 + 索引** | 隔离性好、易管理 | 需维护索引 | ✅ |

### 2. 对话模型配置

**决策**：每个对话独立模型配置

**原因**：
- 用户可能在不同对话中使用不同模型
- 支持对话中途切换模型
- 更灵活的使用场景

**数据结构**：
```rust
Conversation {
  provider_id: String,  // 当前对话使用的 Provider
  model: String,        // 当前对话使用的模型
}
```

### 3. 路由设计

**决策**：嵌套 hash 路由 `#chat/{conversation_id}`

**原因**：
- URL 语义清晰
- 符合 RESTful 风格
- 便于未来扩展

**路由表**：
```
#chat                  → 对话列表（空状态）
#chat/{id}             → 具体对话
#translator            → 翻译器
#settings              → 设置
#lens                  → Lens 窗口（独立）
```

### 4. 启动逻辑

**决策**：动态切换 Dock 图标显示

**实现**：
- 打开 AI 客户端 → `ActivationPolicy::Regular`（显示 Dock）
- 关闭 AI 客户端 → `ActivationPolicy::Accessory`（隐藏 Dock）
- 用户点击图标 → 默认打开 AI 客户端

**原因**：
- 保持工具型应用的轻量感
- AI 客户端使用时像正常应用
- 灵活平衡两种使用模式

### 5. 对话列表分页

**决策**：后端分页加载，初始 50 条

**原因**：
- 对话可能很多（1000+），一次加载全部会卡顿
- 用户主要关注最近的对话
- 性能优化

**API 设计**：
```rust
chat_get_conversations(
  offset: usize,      // 起始位置
  limit: usize,       // 数量
  folder: Option<String>  // 可选的文件夹筛选
)
```

### 6. 消息发送机制

**决策**：新建独立 `chat_send_message` 命令，而非复用 `lens_ask`

**原因**：
- 语义清晰，AI 客户端专用
- 事件分离（`chat-stream` vs `lens-stream`）
- 便于独立扩展功能

**流程**（当前实现；Agent 层标准化见 [CHAT_AGENT_RUNTIME_PRD.md](./CHAT_AGENT_RUNTIME_PRD.md)）：
```
用户发送 → chat_send_message → complete_assistant_reply
                                      ↓
                    chat/model (LanguageModelProvider) + 多轮 tool loop
                                      ↓
                              chat-stream / chat-tool 事件
                                      ↓
                              前端累积显示
```

### 7. UI 组件结构

**决策**：模块化组件，而非单文件

**文件结构**：
```
src/chat/
  ├── Chat.tsx              # 主容器
  ├── Sidebar.tsx           # 左侧边栏
  ├── ConversationList.tsx  # 对话列表
  ├── MessageList.tsx       # 消息列表
  ├── MessageBubble.tsx     # 消息气泡
  ├── InputBar.tsx          # 输入栏
  ├── ModelSelector.tsx     # 模型选择器
  ├── api.ts                # API 调用封装
  ├── types.ts              # 类型定义
  └── utils.ts              # 工具函数
```

**原因**：
- 清晰可维护
- 便于复用
- 符合 React 最佳实践

### 8. 侧边栏设计

**决策**：固定 270px 宽度，可折叠

**布局**：
```
┌─ 顶部 (新建、搜索)
├─ 快捷入口 (ChatGPT、库、GPT)
├─ 对话列表 (按时间分组)
└─ 底部 (设置)
```

### 9. 对话分组逻辑

**决策**：按相对时间自动分组

**分组规则**：
- 今天
- 昨天
- 最近 7 天
- 最近 30 天
- 更早

**实现位置**：前端 `groupConversationsByTime()`

---

## 数据流架构

### 对话创建流程
```
用户点击"新建聊天"
  ↓
Chat.handleNewConversation()
  ↓
chatApi.createConversation()
  ↓
[Tauri IPC]
  ↓
chat_create_conversation (Rust)
  ↓
创建 Conversation 对象
  ↓
save_conversation() → 写入文件系统
  ↓
更新 index.json
  ↓
返回 Conversation
  ↓
前端更新 currentConversation
```

### 消息发送流程
```
用户输入消息 → Enter
  ↓
InputBar.handleSend()
  ↓
Chat.handleSendMessage()
  ↓
chatApi.sendMessage()
  ↓
[Tauri IPC]
  ↓
chat_send_message (Rust)
  ↓
complete_assistant_reply
  ├─ context 估算 / 可选压缩
  ├─ Agent tool loop（规划：带 tools；合成：无 tools）→ chat/model
  ├─ mcp/registry 执行工具（native / skill / mcp）
  └─ 持久化 model_messages + api_messages
  ↓
chat-stream / chat-tool / chat-tool-confirm 事件
  ↓
前端监听器累积内容
  ↓
MessageList 实时显示
  ↓
完成后保存到文件
```

> **说明**：Lens 仍使用 `lens_ask` 与 `lens-stream`；Chat 不复用该路径。

### 对话加载流程
```
组件挂载
  ↓
Sidebar.loadConversations()
  ↓
chatApi.getConversations(0, 50)
  ↓
[Tauri IPC]
  ↓
chat_get_conversations (Rust)
  ↓
读取 index.json
  ↓
排序、分页、筛选
  ↓
返回 ConversationListItem[]
  ↓
前端分组显示
```

---

## 技术选型

### 后端
- **Rust**: 性能、内存安全
- **Tauri v2**: 跨平台、轻量级
- **Serde**: JSON 序列化
- **Tokio**: 异步运行时
- **UUID**: 唯一 ID 生成

### 前端
- **React 18**: 声明式 UI
- **TypeScript**: 类型安全
- **TailwindCSS v4**: 快速样式开发
- **Lucide Icons**: 现代图标库
- **Vite**: 快速构建

---

## 性能优化策略

### 1. 分页加载
- 初始只加载 50 条对话
- 滚动加载更多
- 减少首屏渲染压力

### 2. 索引文件
- 对话列表只包含元数据
- 点击对话才加载完整消息
- 减少内存占用

### 3. 流式响应
- 边接收边显示
- 用户体验更流畅
- 减少等待时间

### 4. 惰性加载
- 使用 `React.lazy()` 按需加载组件
- Chat 组件只在访问 `#chat` 时加载
- 减少初始 bundle 大小

---

## 未来扩展点

### 短期（1-2 个月）
1. Lens 截图集成
2. 对话搜索
3. 文件夹管理
4. 附件支持

### 中期（3-6 个月）
1. 多模型对比
2. Prompt 模板库
3. 对话导出/导入
4. 快捷键系统

### 长期（6+ 个月）
1. Agent 模式
2. 知识库/RAG
3. 插件系统
4. 云端同步

---

## 兼容性考虑

### macOS
- ✅ 动态切换 ActivationPolicy
- ✅ Dock 图标显示/隐藏
- ✅ 全局快捷键

### Windows
- ⚠️ 没有 ActivationPolicy，托盘常驻
- ✅ 全局快捷键
- ✅ 任务栏显示

### 深色模式
- ✅ 跟随系统
- ✅ 手动切换
- ✅ 所有组件适配

---

## 代码规范

### Rust 命名
- 命令函数：`chat_xxx`（snake_case）
- 类型：`Conversation`（PascalCase）
- 模块：`chat`（snake_case）

### TypeScript 命名
- 组件：`Chat`（PascalCase）
- 函数：`handleSendMessage`（camelCase）
- 类型：`ConversationListItem`（PascalCase）
- 常量：`DEFAULT_LIMIT`（UPPER_CASE）

### 文件命名
- 组件：`Chat.tsx`（PascalCase）
- 工具：`utils.ts`（camelCase）
- API：`api.ts`（camelCase）

---

## Chat 运行时分层（Model + Agent）

| 层 | 目录/模块 | 职责 |
|----|-----------|------|
| **Model** | `src-tauri/src/chat/model/` | Provider 协议：`GenerateRequest`、`StreamPart`、`LanguageModelProvider` |
| **Agent**（规划中） | `src-tauri/src/chat/agent/` | Tool loop、Step、prepare/stop/finish；详见 [CHAT_AGENT_RUNTIME_PRD.md](./CHAT_AGENT_RUNTIME_PRD.md) |
| **Tools** | `src-tauri/src/mcp/`、`native_tools/` | 工具定义、MCP 协议、内置工具执行 |
| **Orchestration** | `src-tauri/src/chat/commands.rs` | Tauri 命令、会话 IO、事件 emit |

规范索引：

- Provider 契约：`src-tauri/src/chat/model/README.md`

---

## 测试计划

### 单元测试
- [ ] 工具函数（`groupConversationsByTime`、`formatRelativeTime`）
- [ ] 存储函数（`save_conversation`、`load_conversation`）

### 集成测试
- [ ] 对话创建 → 消息发送 → 对话加载
- [ ] 流式响应完整流程
- [ ] 对话删除后索引更新

### 端到端测试
- [ ] 用户完整使用流程
- [ ] 多窗口场景
- [ ] 异常情况处理

---

## 风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 对话文件损坏 | 高 | 定期备份、错误隔离 |
| 索引不同步 | 中 | 启动时校验、自动修复 |
| 流式响应中断 | 中 | 超时重试、错误提示 |
| 磁盘空间不足 | 低 | 提示清理、压缩历史 |

---

**文档版本**: v1.1  
**最后更新**: 2026-06-04  
**维护者**: ZMGID
