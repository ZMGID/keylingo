# n8n Editor UX（官方文档）

**Date:** 2026-08-29  
**Scope:** n8n 编辑器交互模型；简化 clone 该保留 / 该丢掉什么。  
**Sources:** 仅 `docs.n8n.io`（GitBook 现行路径；旧 URL `/workflows/`、`/flow-logic/`、`/data/data-structure/` 已迁到 `/build/`）。  
**Not sources:** 博客、论坛、二手评测。文档未写死的细节标为「文档未明示」。

文档里几乎不用 **NDV** 这个缩写，用户可见说法是 **node details view** / **detail view** / **node editor view**。下文 NDV 一律指这个面板。

---

## 路径迁徙（用户点名的旧 URL）

| 旧 URL | 现行官方页 |
| --- | --- |
| https://docs.n8n.io/workflows/ | https://docs.n8n.io/build/understand-workflows/ · https://docs.n8n.io/build-your-first-workflow |
| https://docs.n8n.io/flow-logic/ | https://docs.n8n.io/build/flow-logic/ |
| https://docs.n8n.io/data/data-structure/ | https://docs.n8n.io/build/work-with-data/understand-n8ns-data-structure/ |
| Schedule Trigger | https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.scheduletrigger/ |
| Manual Trigger | https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.manualworkflowtrigger/ |

---

## 1. 用户核心循环

官方 quickstart 把「第一次搭 workflow」写成固定六步，不是口号。

**循环（与文档逐步对应）：**

1. **Create workflow** → 空 Canvas 上只有 **Add first step**（不是随便拖 Action）。
2. **选 Trigger**（Schedule / App Event / Manual…）→ n8n **立刻打开 NDV**。
3. **Add node connector**（节点右侧）→ 搜 Node → 选 **operation**（Trigger 有闪电标，Action 是任务）→ 再开 NDV。
4. 需要时在 NDV 里 **Create credential**（保存时 n8n 会测连通）。
5. 从 **INPUT pane** 拖字段进参数；必要时切 **Expression**（`{{ $today.minus(7, 'days') }}`、`{{$json["classType"]}}`）。
6. **Execute step**：只跑这一步（及填 input 所需的上游），在 **OUTPUT** 看表/JSON。没跑过上游就没有可拖的数据。
7. 关 NDV 回 Canvas；IF 的 **true/false connector** 分叉。
8. Canvas 底栏 **Execute Workflow** 整图试跑，Canvas 上逐步高亮。
9. 要定时/收 webhook：**Publish**。未 Publish 只能手动跑。

来源：

- https://docs.n8n.io/build-your-first-workflow
- https://docs.n8n.io/build/understand-workflows/create-and-run-workflows
- https://docs.n8n.io/build/understand-workflows/workflow-components/work-with-nodes
- https://docs.n8n.io/build/understand-workflows/understand-executions/types-of-executions
- https://docs.n8n.io/build/understand-workflows/save-and-publish-workflows

**产品含义：** 「直觉」来自 **Trigger 起手 + 边加边测（Execute step）+ INPUT/OUTPUT 同屏 + 最后才 Publish**。不是来自节点数量。

---

## 2. Canvas 语法：哪些是 load-bearing

Canvas 定义：*the main interface for building workflows in n8n's editor UI. You use the canvas to add and connect nodes to compose workflows.*  
https://docs.n8n.io/build/understand-workflows/create-and-run-workflows

| 语法 | 官方怎么写 | 对「图形直觉」 | 简化 clone |
| --- | --- | --- | --- |
| 左进右出 | 右侧 grey dot / **Add node** 拉到下一节点左侧 grey rectangle | **Load-bearing** | **Keep** |
| 空画布只加 Trigger | **Add first step** 打开的是 **trigger nodes** 面板 | **Load-bearing** | **Keep** |
| 不是「只能一个 Trigger」 | 生产需要 ≥1 个 **非 Manual** 的 Trigger；**Manual Trigger 全局只能一个**；多 Trigger 要自己 merge 分支 | 单 Trigger 更直觉；多 Trigger 是进阶 | **默认一个**；多 Trigger 可后置 |
| Ports | IF：true/false；Switch：多 output；Merge：多 input；On Error 可走 error output | **分叉时 load-bearing** | **Keep IF 双口**；Switch/Merge 可后置 |
| 节点右侧 + | **Add node connector**；也可拖线 | **Load-bearing**（线性搭积木） | **Keep** |
| 边上的删除 | Hover connection → Delete | 次要 | Keep 即可 |
| Sticky notes | Core node；模板里用来教别人 | 注释，不是执行模型 | **Drop（v1）** |
| Hover：Execute / Deactivate / Delete | Execute step、Deactivate、context（Pin、Duplicate、Tidy up、Convert to sub-workflow） | **Execute + Deactivate 是测试闭环** | **Keep 这两项**；Pin/sub-workflow 可后置 |
| Canvas Groups | 2.28+；Trigger **不能**进组 | 大图整理 | **Drop（v1）** |
| 分支执行序 | v1：先完整一条分支；上→下，同高则左→右 | 多分支才需要知道 | 文档化即可，UI 不必暴露 v0/v1 |

连接：https://docs.n8n.io/build/understand-workflows/workflow-components/connect-nodes-together  
Hover：https://docs.n8n.io/build/understand-workflows/workflow-components/work-with-nodes  
Sticky：https://docs.n8n.io/build/understand-workflows/workflow-components/add-notes-and-documentation  
Groups：https://docs.n8n.io/build/understand-workflows/workflow-components/canvas-groups  
Manual 唯一：https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.manualworkflowtrigger  
多 Trigger merge：https://docs.n8n.io/build/integrate-ai/test-and-improve-ai-workflows/fix-common-issues  
执行序：https://docs.n8n.io/build/flow-logic/understand-execution-order

**NDV（文档用语，非 NDV 缩写）**

- 双击 / Enter 打开；加节点后常自动打开。
- **Parameters** + **Settings**；**INPUT** / **OUTPUT**；Table 与 JSON。
- **Fixed | Expression**；空参数按 `=` 进 expression。
- **Execute step** 在 NDV 内。
- Settings：Always Output Data、Execute Once、Retry On Fail、On Error（Stop / Continue / Continue using error output）。

https://docs.n8n.io/build/keyboard-shortcuts · https://docs.n8n.io/build/understand-workflows/workflow-components/work-with-nodes · https://docs.n8n.io/build/work-with-data/reference-data/use-the-ui-mapper

---

## 3. Node 分类与最小集

官方两套分类，不要混成「Trigger vs Action vs Core vs App」四选一。

**按 operation（加节点时）：**

- **Triggers**：事件/条件启动；搜索带 bolt。
- **Actions**：拉数、改数、调外部、再触发别的系统。

https://docs.n8n.io/integrations/builtin/node-types  
https://docs.n8n.io/build/understand-workflows/workflow-components/work-with-nodes

**按库（Built-in）：** Core / App / Trigger（文档单独成栏）/ Cluster / Credentials / Community。  
**Core** 可以是 Trigger 也可以是 Action：逻辑、调度、通用 API，不绑某一家 SaaS。

https://docs.n8n.io/integrations/builtin  
https://docs.n8n.io/integrations/builtin/node-types

**生产 Trigger 规则：** *All production workflows need at least one trigger*；Publish 时「any trigger other than the manual trigger」。  
https://docs.n8n.io/build/understand-workflows/understand-executions/types-of-executions  
Schedule 必须 save + publish：https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.scheduletrigger

### 简化产品最小集

| 层 | Keep | 官方角色 |
| --- | --- | --- |
| Trigger | **Manual Trigger** | 只手跑、不能自动；一个 workflow 只能一个 |
| Trigger | **Schedule Trigger** | 表单间隔优先，Cron 进阶 |
| Trigger | **Webhook** | 无专用 App Trigger 时的入口；Test URL vs Production URL |
| Action / 万能出口 | **HTTP Request** | 任意 REST；可 import curl |
| Core 逻辑 | **If** | 双口；多于两路再用 Switch |
| Core 数据 | **Edit Fields (Set)** | 官方推荐的可视化改字段；默认 Manual Mapping 不是 JSON |
| Core 可选 v1.5 | **Switch**、**Merge**（Append 即可）、**No-op** | 多路 / 合流 / 多 Trigger 对齐 |

**明确后置 / Drop：** 文档里整库 App nodes、Cluster（root+sub）、Community、Code 当默认变换、Loop Over Items（多数节点已按 item 隐式循环）、Error Trigger 独立 error workflow、Execute Sub-workflow、Compare Datasets、Merge SQL、AI Transform（仅 Cloud）。

文档未给出「400+ apps」精确数字；sitemap 里 App/Core 节点页是数百量级。复杂度来自 **目录宽度**，不是某一个 Node。

---

## 4. 数据传递：强大且难的部分

**结构：** 节点之间是 **object 数组**。每项通常 `{ "json": { … }, "binary": { … } }`。  
https://docs.n8n.io/build/work-with-data/understand-n8ns-data-structure

**隐式循环：** 节点收到 N 个 item，就对每个做一次配置好的 operation（Trello 建 N 张卡）。这是图形产品「看起来没 loop 也能处理列表」的原因，也是新手以为「跑一次 = 一个 JSON」会翻车的原因。  
同页 + https://docs.n8n.io/build/flow-logic/loop （*n8n handles this repetitive processing automatically*）

**Expressions：** `{{ … }}` 里是 JS-like；`$json`、`$('NodeName')`、`$today` 等。拖 INPUT 生成 `{{ $json.fruit }}`。官方：**能写 expression 就写，因为有即时 preview**。复杂变换应先 **Edit Fields**，不要把长表达式散落各参数。  
https://docs.n8n.io/build/work-with-data/transform-data/expressions-for-data-transformation  
https://docs.n8n.io/build/work-with-data/expressions-versus-data-nodes  
https://docs.n8n.io/build/work-with-data/reference-data/use-the-ui-mapper

**更难的一层：item linking。** 每个 output item 链回产生它的 input。拆/合之后 Code/自研节点必须自己保链，否则编辑器报错。  
https://docs.n8n.io/build/work-with-data/reference-data/link-data-items  
https://docs.n8n.io/build/work-with-data/reference-data/link-data-items/how-items-link-through-workflows

**结论：** JSON items + `{{ $json }}` **既是能力也是陡坡**。n8n 自己已经提供更简单的替代，只是没设成唯一路径：

| 替代（文档已有） | 简化 clone 怎么用 |
| --- | --- |
| UI mapper 拖放 | **默认映射**；用户不先看到 `{{ }}` |
| Edit Fields Manual Mapping；Fixed vs Expression | **默认表单**；JSON Output / Expression **opt-in** |
| Aggregate / Split Out / Limit / Sort / Remove Duplicates | 按需，不要一上来 Code |
| AI Transform（自然语言 → 只读 code，Cloud） | 可选「帮我映射字段」；不要当核心运行时 |
| Pin + 可编辑 pinned JSON | 开发时冻住样本；**生产忽略 pin** |

命名 output / AI map fields / 无 expression：**可行，而且对齐官方分层**（form → expression → Code → AI）。不要在 v1 复制完整 expression 语言和 item linking。

---

## 5. Execution 模型：简化版也必须有

三档（官方用词）：

| 档 | 行为 | 必须？ |
| --- | --- | --- |
| **Manual executions** | Canvas **Execute Workflow**；可 pin；用来迭代看数据 | **Must** |
| **Partial executions** | NDV **Execute step** = 该节点 + 填 input 的上游；可 Deactivate 跳过副作用 | **Must**（没有逐步验数据，图形编辑就死） |
| **Production executions** | **Publish** + 非 Manual Trigger；**不在 Editor 上回放**，去 **Executions** | **Must**（否则无法区分草稿和线上） |

约束：

- 新 workflow **默认 unpublished**；未 Publish 只能手动。
- Publish **锁到一个 version**；之后编辑是 draft，生产仍跑已发布版。
- Partial 也要连着 Trigger（否则 *destination node is not connected to any trigger*）。
- Webhook：**Test URL**（Listen / 未 active 时 Execute）把数据画在编辑器；**Production URL** 在 Publish 后注册，编辑器不刷，去 Executions 看。
- Pin **仅开发**；生产忽略。
- Dirty node：改参/插节点后输出算 stale，需再 Execute。
- Executions list：按 workflow；Filter status；失败可 Retry（当前图或当时图）；失败 **Debug in editor** / 成功 **Copy to editor**（把数据 pin 到首节点）。Debug 有套餐门槛。
- 配额：手工跑、error workflow、sub-workflow **不计** production quota。

来源：

- https://docs.n8n.io/build/understand-workflows/understand-executions
- https://docs.n8n.io/build/understand-workflows/understand-executions/types-of-executions
- https://docs.n8n.io/build/understand-workflows/save-and-publish-workflows
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.webhook
- https://docs.n8n.io/build/work-with-data/pin-and-mock-data
- https://docs.n8n.io/build/understand-workflows/understand-executions/understand-dirty-nodes
- https://docs.n8n.io/build/understand-workflows/understand-executions/view-executions-for-a-single-workflow
- https://docs.n8n.io/build/understand-workflows/understand-executions/debug-executions
- https://docs.n8n.io/build/manage-workflows/configure-workflow-settings

**简化版可丢：** named versions、workflow review 门禁、custom execution data、OpenTelemetry span、redaction 策略 UI、v0/v1 execution order 开关。  
**简化版仍要：** 草稿 vs 已发布、整图手动跑、单步 + 可见 IO、至少失败 Execution 列表（时间、状态、能否点开看数据）。Pin 很值，不是必须第一周。

---

## 6. 为什么压人：复杂度来源排序

（按「新用户会先撞墙」排，不是按代码量。）

1. **Item 数组 + 隐式 per-item 执行 + item linking**  
   一次执行变 N 次 API；`$json` 其实是「当前 item」。文档把自动循环当特性，用户当 bug。

2. **Expression 语言**  
   JS + `$json` / `$('Node')` / IIFE；*Can't get data for expression* / *Referenced node is unexecuted*。拖放能生成，修的时候仍要懂语法。

3. **App 目录 × Credentials**  
   每家一套 auth；OAuth vs key；还可在 credential 里写 expression。HTTP Request 本可当退路，但目录太大，用户先迷路。

4. **Test / Production 分裂**  
   两套 Webhook URL；pin 线上无效；生产不在 Canvas 播；Schedule 不 Publish 就不跑。心智是「保存即上线」的人会懵。

5. **分支 + Merge + 执行序**  
   IF/Switch 好懂；Merge 的 join 语义、旧版 If+Merge 双支都会跑、多 Trigger 还要对齐 schema。

6. **Error 作为另一条 workflow**  
   Settings → Error workflow → 必须以 **Error Trigger** 开头。节点级 On Error 已经够绕。

7. **Publish 版本 / Review / Canvas Groups / Cluster AI 节点**  
   团队与大图功能；不是第一小时阻塞，但顶栏会一直在。

8. **Sticky notes、Tidy up、键盘快捷键**  
   表层 chrome，不是陡坡。

「400+ apps」是目录焦虑的通俗说法；官方页用的是 *built-in integrations* / node library，未在已读页写死数字。

---

## 7. 更直觉的简化 n8n：五条 UX 规则（可验收）

1. **空 Canvas 只允许 Trigger picker（Add first step），禁止从 Action 目录起手。**  
   对齐 https://docs.n8n.io/build/understand-workflows/workflow-components/work-with-nodes 与 first-workflow。生产路径再强制：Schedule / Webhook 必须 Publish 才活。

2. **NDV 默认是表单（Credential 下拉 + 字段），不是 JSON 编辑器。**  
   对齐 Edit Fields 的 **Manual Mapping** 默认、HTTP 的 Name/Value。JSON / Expression / Code 用 **Expression** 或「高级」才出现。空框 `=` 进表达式可以留，但不能当首页。

3. **配置任何依赖上游的字段前，必须先有样本数据：Execute step，或 Pin/Mock。**  
   官方 quickstart 写明：没 Execute NASA 就拖不到 `classType`。映射 = 从 OUTPUT 拖到表单，生成 **字段绑定**；底层可以仍是 `{{ $json.x }}`，界面默认不展示。

4. **测试与上线永远是两个按钮、两套语义。**  
   Canvas：**Execute Workflow** / **Execute step** = 草稿，数据留在编辑器。顶栏：**Publish** = 启用 Trigger（Schedule 开始走、Webhook 换 Production URL）。Webhook 在 NDV 顶上并排 **Test URL | Production URL**，文案写清谁会写进编辑器、谁只进 Executions。

5. **Canvas 语法锁死为：左输入、右输出、右侧 + 加下一步；IF 只有 true/false 两口；hover 只有 Execute step 与 Disable。**  
   不要在 v1 做 Canvas Groups、Sticky、Convert to sub-workflow、Merge SQL、多 Trigger。多路用第二个 If 或以后再加 Switch。App 集成默认 **HTTP Request + curl 导入**，而不是先做集成市场。

---

## Keep vs Drop（一页）

**Keep（直觉来源）**

- Trigger-first；左→右；右侧 +；连接线
- NDV = 表单 + INPUT/OUTPUT + Execute step
- 拖放映射（可隐藏 expression 字符串）
- Execute Workflow vs Publish
- 每 workflow 的 Executions（至少失败）
- Manual / Schedule / Webhook + HTTP Request + If + Edit Fields
- Credential 挂在节点上（OAuth/key 表单，不必动态 expression）
- 节点 Disable；Webhook test/prod URL

**Drop 或后置（压人来源）**

- 大 App 目录、Community、Cluster
- 默认暴露 `{{ $json }}` / Code / item linking
- Error workflow 整图、Stop and Error
- Canvas Groups、Sticky、sub-workflow 转换
- Named versions、Review、execution order v0
- Merge 全套 join、Loop Over Items（先靠隐式 per-item，并在 UI 写清「对每一条」）
- Pin 编辑 JSON（有价值，可 v1.5）
- AI Transform 作为必装运行时

---

## 附录：已读官方 URL

- https://docs.n8n.io/build-your-first-workflow
- https://docs.n8n.io/build/understand-workflows
- https://docs.n8n.io/build/understand-workflows/create-and-run-workflows
- https://docs.n8n.io/build/understand-workflows/create-and-edit-credentials
- https://docs.n8n.io/build/understand-workflows/workflow-components/work-with-nodes
- https://docs.n8n.io/build/understand-workflows/workflow-components/connect-nodes-together
- https://docs.n8n.io/build/understand-workflows/workflow-components/add-notes-and-documentation
- https://docs.n8n.io/build/understand-workflows/workflow-components/canvas-groups
- https://docs.n8n.io/build/understand-workflows/understand-executions
- https://docs.n8n.io/build/understand-workflows/understand-executions/types-of-executions
- https://docs.n8n.io/build/understand-workflows/understand-executions/understand-dirty-nodes
- https://docs.n8n.io/build/understand-workflows/understand-executions/debug-executions
- https://docs.n8n.io/build/understand-workflows/understand-executions/view-executions-for-a-single-workflow
- https://docs.n8n.io/build/understand-workflows/save-and-publish-workflows
- https://docs.n8n.io/build/manage-workflows/configure-workflow-settings
- https://docs.n8n.io/build/keyboard-shortcuts
- https://docs.n8n.io/build/flow-logic
- https://docs.n8n.io/build/flow-logic/split-with-conditionals
- https://docs.n8n.io/build/flow-logic/merge-data
- https://docs.n8n.io/build/flow-logic/loop
- https://docs.n8n.io/build/flow-logic/handle-errors-gracefully
- https://docs.n8n.io/build/flow-logic/understand-execution-order
- https://docs.n8n.io/build/work-with-data
- https://docs.n8n.io/build/work-with-data/understand-n8ns-data-structure
- https://docs.n8n.io/build/work-with-data/expressions-versus-data-nodes
- https://docs.n8n.io/build/work-with-data/reference-data/use-the-ui-mapper
- https://docs.n8n.io/build/work-with-data/transform-data/expressions-for-data-transformation
- https://docs.n8n.io/build/work-with-data/pin-and-mock-data
- https://docs.n8n.io/build/work-with-data/reference-data/link-data-items
- https://docs.n8n.io/build/work-with-data/reference-data/link-data-items/how-items-link-through-workflows
- https://docs.n8n.io/integrations/builtin
- https://docs.n8n.io/integrations/builtin/node-types
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.scheduletrigger
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.manualworkflowtrigger
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.webhook
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.if
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.switch
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.merge
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.set
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.httprequest
- https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.aitransform
- https://docs.n8n.io/build/integrate-ai/test-and-improve-ai-workflows/fix-common-issues
- https://docs.n8n.io/sitemap.md
