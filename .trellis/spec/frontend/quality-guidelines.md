# Quality Guidelines

> Code quality standards for frontend development.

---

## Overview

前端**没有 e2e 兜底**（见 CLAUDE.md）。`cargo test` 覆盖 Rust 侧，`loop_tests.rs`
覆盖 agent 循环，但都帮不到前端的 props 传递与事件时序。因此纯结构重构的正确性
只能靠三层叠加：`typecheck` + `lint` + **组件/hook 级 Vitest**。

---

## Forbidden Patterns

<!-- Patterns that should never be used and why -->

- **从组件文件（`.tsx`）导出常量或普通函数**：触发
  `react-refresh/only-export-components`，破坏 Fast Refresh。共享常量单列 `.ts`
  模块（`settings/memoryLayers.ts`、`settings/uiFont.ts`）。类型导出不受限。
- **`fill: both` 配 `transform` 动画作用在含 `fixed` 后代的容器上**：动画结束后
  残留的 `translateY(0)` 计算值是 `matrix(1,0,0,1,0,0)` 而非 `none`，足以成为
  `fixed` 后代的包含块，遮罩不再铺满窗口。用 `fill: backwards`。

---

## Required Patterns

<!-- Patterns that must always be used -->

- **纯结构重构必须增量提交**：一个搬迁单位一个 commit，每个都独立可 `git revert`。
  不做跨单位的公共抽象 —— 那会让 commit 互相依赖，破坏单步回滚。
- **搬迁后核对保真度**，不靠目测。可机械比对的量：
  ```bash
  # JSX 元素计数：新文件 vs 旧块
  grep -c '<SettingRow' new.tsx
  git show HEAD:old.tsx | sed -n '2259,2575p' | grep -c '<SettingRow'
  ```
  handler 主体行数、依赖数组原文同样要逐一比对 —— 后者任一变动都会改执行时机。

---

## Testing Requirements

新增或搬迁的组件 / hook 需要测试，覆盖点是**typecheck 抓不到的那类错**：

- props 类型对但接错字段（`onUpdateLens` vs `onUpdateSettings`，签名相同）
- 回调走错 updater（L2 草稿写进 L1）
- 条件渲染的开关联动（总开关关闭时下游整组是否消失）
- 异步订阅的卸载竞态

### 断言写完必须做变异验证

**这是硬要求，不是可选项。** 往被测代码里注入一个 typecheck 会放行的真实错误，
确认测试**会失败**；然后还原。

实测有效：为合帧 hook 写了 13 个断言后，注入「去掉 rAF 节流判断」和「去掉入队
会话校验」两个错误，测试**照样全绿** —— 补了「只排一次 rAF」「后台会话不覆盖当前
挂起帧」两条断言才抓住。没有这一步会误以为覆盖到了性能关键路径。

批量做法：
```python
for old, new, desc in mutations:
    s = open(path).read(); assert old in s
    open(path, 'w').write(s.replace(old, new, 1))
    r = subprocess.run(['npx','vitest','run', test_path], capture_output=True, text=True)
    print(('✓ 抓到' if 'failed' in r.stdout else '✗ 漏掉'), desc)
    shutil.copy(backup, path)   # 必须还原
```

### 测试环境

`vite.config.ts` 只给 `*.test.tsx` 配了 jsdom。`.ts` 测试若需要 DOM，在文件头加：

```ts
/** @vitest-environment jsdom */
```

### 已知的断言陷阱

从实际失败中记录，避免重复踩：

- `HotkeyInput` 把值渲染成**按键徽章**（且修饰键随平台变形），不是 `<input>` ——
  不能用 `getByDisplayValue`
- `Select` 是自绘的 button + 弹出菜单，**不是原生 `<select>`** ——
  不能用 `selectOptions`，要先点开再选条目
- `Toggle` 的 role 是 `switch`
- i18n 里有同文案不同键：`removeKey` 与 `removeModel` 都是「移除」，
  `hotkeyClear` 是「清空快捷键」—— 同名按钮按 CSS class 区分，不要靠 name

### Fixture

`Settings` 有 60+ 字段，逐个填会让测试脆弱（加字段就要改所有 fixture）。
用 `settings/tabs/testFixtures.ts` 的 `makeSettings(overrides)`：只给被测组件
真正读到的字段，其余按 `Partial` 断言掉。缺失字段若被误读会立刻 `undefined` 报错，
反而比填假值更能暴露问题。

---

## Code Review Checklist

<!-- What reviewers should check -->

纯结构重构的 diff 审阅要点：

- [ ] 是否夹带了非搬迁改动（顺手修 bug / 调样式 / 改文案）？发现应另开任务
- [ ] 依赖数组是否有变动？若有，新增项是否为 `useCallback` 稳定身份
- [ ] 是否有「刻意的例外」被统一处理掉了？改之前读注释
  （例：`cancelCurrentRunLocally` 刻意保留快照以冻结展示）
- [ ] 新增测试是否做过变异验证
