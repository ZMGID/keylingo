# Design — 拆分 SettingsShell.tsx 的 tab 面板

## 边界

**动的**：`SettingsShell.tsx` 主 `return` 里 9 个 `activeTab === 'x' && (...)` 分支的 JSX。
**不动的**：107 个 hook 调用、`settingsModals`、`categoryNav`、`loading`/`loadError` 早退分支、1742 行的 memory 数据准备、全部 CSS。

## 目标形态

```
src/settings/
  SettingsShell.tsx        # 状态层 + 布局骨架 + tab 路由（~2000 行）
  tabs/
    ProvidersTab.tsx       # 319 行
    ChatTab.tsx            # 227
    GeneralTab.tsx         # 210
    AboutTab.tsx           # 176（条件：props ≤ 10）
    MixerTab.tsx           # 142
    LensTab.tsx            # 140
    HotkeysTab.tsx         # 111
    MemoryTab.tsx          # 92
    TranslateTab.tsx       # 71
```

shell 侧调用点形态统一：

```tsx
{activeTab === 'providers' && (
  <ProvidersTab
    settings={settings}
    onUpdate={updateSettings}
    lang={lang}
    t={t}
    onOpenModelPicker={setModelPickerProviderId}
    onOpenModelTest={setModelTestProviderId}
    onConfirmDelete={setConfirmDeleteProviderId}
    onOpenDrawer={setDrawerModel}
    onToggleGzipInfo={setGzipInfoOpen}
    selectedProviderId={selectedProviderId}
    onSelectProvider={setSelectedProviderId}
  />
)}
```

## 契约

每个 tab 组件：

- 是**纯展示组件**：不含 `useEffect`、不发 IPC、不做数据加载。允许纯 UI 局部状态（如输入框草稿的 `useState`）**仅当**它原本就在 JSX 内联位置存在。
- props 显式命名，回调用 `on*` 前缀（shell 侧的 `setXxx` 在 props 里改名为 `onXxx`，让组件不知道对面是个 setState）。
- 只接收自己用到的 `settings` 子树，不是整个 `settings` —— 除非该 tab 真的横跨多个顶层字段（`general`、`about` 是这种情况，接整个 `settings`）。

## 为什么不下沉状态（已确认的决策）

107 个 hook 里大量是跨 tab 共享的：`settings` 草稿、`initialSettingsSnapshot`（dirty 判定）、`currentSettingsSnapshotRef`（stale-while-revalidate 的并发保护）、各种下载状态机。

下沉会带来两个真实风险：

1. **dirty 判定失真**。保存栏的 `hasUnsavedChanges` 依赖 shell 持有的完整快照对比。状态散到 tab 里后，切走再切回会重挂载、局部状态丢失，dirty 计算就不再可信。
2. **stale-while-revalidate 竞态**。`currentSettingsSnapshotRef` 用来防止后台校准覆盖用户正在编辑的草稿（见 SettingsShell.tsx:621 一带的注释）。这个 ref 必须是单一来源。

所以本次只搬 JSX。状态下沉如果将来真有必要，是独立一次重构，前提是先把 dirty/校准这套机制抽成一个 `useSettingsDraft` hook。

## 逐步顺序与理由

按「耦合从低到高」排，让方法在低风险 tab 上验证成熟：

1. `hotkeys`(0)、`lens`(0)、`mixer`(0) — 零 shell 回调，纯搬迁，验证流程
2. `translate`(2)、`memory`(4) — 少量回调，验证 props 命名约定
3. `chat`(2)、`providers`(5) — 大块，但耦合可控
4. `general`(7) — 耦合偏高
5. `about`(9) — 最后，且有退出条件

## 回滚

一个 tab 一个 commit，`git revert <sha>` 即可单独退回。不做跨 tab 的公共抽象（比如「所有 tab 共用一个 BaseTabProps」）—— 那会让 commit 互相依赖，破坏单步回滚。

## 已知风险

| 风险 | 处理 |
|------|------|
| `about` props 爆炸（9 个回调 + 状态机） | 硬性退出条件：props > 10 就放弃该 tab，记录原因 |
| 抽取时误改 JSX 结构导致 CSS 选择器失配 | CSS 里有 `.settings-embedded-nav-item` 等结构选择器；冒烟必须逐 tab 目视 |
| props 传递遗漏导致运行时 undefined | typecheck 能抓住（TS strict），但 optional props 不会报错 —— props 类型里避免滥用 `?` |
| 无 e2e 兜底 | 每 commit 后手动点开对应 tab，改一项 + 保存 |
