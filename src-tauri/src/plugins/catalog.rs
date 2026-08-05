//! 内置插件目录：广场条目 + **给 AI 的安装规范** + 启用后注入的 Skill/MCP/提示。
//!
//! 安装由 Kivio Agent 按 `install_doc` 执行（run_command），**不是**后端静默下载。

/// 插件附带的 stdio MCP 规格：启用时挂到 chatTools.servers，关闭时禁用/断连。
#[derive(Debug, Clone)]
pub struct PluginMcpSpec {
    /// 传给二进制的参数，如 `["mcp"]` → `officecli mcp`（stdio JSON-RPC）
    pub args: &'static [&'static str],
}

#[derive(Debug, Clone)]
pub struct CatalogPlugin {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub binary: &'static str,
    pub tags: &'static [&'static str],
    pub homepage: &'static str,
    pub repo: &'static str,
    /// 官方安装器常用落点（支持 `%LOCALAPPDATA%` 等环境变量）；PATH 未刷新时仍可检测到
    pub known_binary_paths: &'static [&'static str],
    /// 官方 README 原文 URL（raw），安装时 **必须先 web_fetch 阅读**；权威安装/用法以 README 为准
    pub readme_urls: &'static [&'static str],
    /// 短 system 提示（仅 enabled 时注入）
    pub system_hint: &'static str,
    /// 本插件拥有的 skill id 列表（启用才可见）
    pub skill_ids: &'static [&'static str],
    /// 附属 Skill 正文（启用时写入磁盘并进入技能扫描）
    pub skill_md: &'static str,
    /// 可选：从该 GitHub 仓库 / 直链 zip 下载附属 Skill（启用时下载进插件 skill 目录，随上游更新）。
    /// 与 `skill_md` 二选一；两者皆空且非 officecli 时启用不落 Skill。
    pub skill_download_url: Option<&'static str>,
    /// 可选 MCP：启用时自动注册 stdio server
    pub mcp: Option<PluginMcpSpec>,
    /// **Kivio 安装契约**（薄层）：流程/约束/验收；具体命令以 README 为准，勿与 README 冲突
    pub install_doc: &'static str,
}

pub const PLUGIN_CATALOG: &[CatalogPlugin] = &[CatalogPlugin {
    id: "officecli",
    name: "OfficeCLI",
    description: "面向 AI Agent 的 Word / Excel / PowerPoint CLI。单二进制、无需安装 Office。附带 Skill 与 MCP。",
    binary: "officecli",
    tags: &["Word", "Excel", "PowerPoint", "CLI", "Skill", "MCP"],
    homepage: "https://officecli.ai",
    repo: "https://github.com/iOfficeAI/OfficeCLI",
    // 官方 Windows 安装器默认目录；macOS/Linux 常见用户 bin
    known_binary_paths: &[
        r"%LOCALAPPDATA%\OfficeCLI\officecli.exe",
        r"%USERPROFILE%\.local\bin\officecli",
        r"%USERPROFILE%\bin\officecli",
        "/usr/local/bin/officecli",
        "/opt/homebrew/bin/officecli",
    ],
    readme_urls: &[
        // 中文优先；失败再读英文。安装与用法以仓库 README 为权威来源。
        "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/README_zh.md",
        "https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/README.md",
    ],
    // Kivio 适配策略（英文）：仅保留 officecli 专属约束（MCP 优先、禁 watch、禁 mcp <ide>）+ skill 路由表；
    // 通用能力（图直喂 R1、临时目录/清理/绝对路径 R3、bash R4）已下沉到运行时，不再靠 hint 打补丁。
    system_hint: "\
### OfficeCLI (plugin: officecli)\n\
**Role.** Create/read/edit .docx / .xlsx / .pptx with OfficeCLI. Prefer this plugin over python-docx / openpyxl / python-pptx.\n\
\n\
**Skills (official set is installed).** Before substantial work, activate the matching skill (`skill` tool, or MCP `load_skill <cli-name>`):\n\
- Base strategy → `officecli`\n\
- Slides / deck → `officecli-pptx` (`load_skill pptx`); fundraising pitch → `officecli-pitch-deck`; morph motion → `morph-ppt` / `morph-ppt-3d`\n\
- Word → `officecli-docx`; academic paper → `officecli-academic-paper`; fillable form → `officecli-word-form`\n\
- Excel → `officecli-xlsx`; dashboard → `officecli-data-dashboard`; financial model → `officecli-financial-model`\n\
Do **not** start layout-heavy work without the domain skill.\n\
\n\
**Do NOT:**\n\
- Run `officecli` via bash / run_command — always use the MCP `officecli` tool (one persistent process holds the document warm; bash cold-starts a new process per call and skips Kivio's live preview).\n\
- Run `officecli watch` / `unwatch` (MCP edits don't drive watch; Kivio provides its own preview).\n\
- Run `officecli mcp claude|cursor|vscode|…` (Kivio already registers the official stdio server as plugin-officecli).\n\
\n\
**Done.** Tell the user the final saved file path(s).",
    // 官方 skill id = load_skill frontmatter `name`（启用时从 CLI 全量同步）
    skill_ids: OFFICECLI_OFFICIAL_SKILL_IDS,
    skill_md: "", // 空 = 使用官方 CLI 同步（见 install::sync_officecli_official_skills）
    skill_download_url: None,
    mcp: Some(PluginMcpSpec { args: &["mcp"] }),
    install_doc: OFFICECLI_INSTALL_DOC,
}, CatalogPlugin {
    id: "ego-lite",
    name: "ego lite",
    description: "面向 AI Agent 的 Chromium 浏览器（macOS）。Agent 在独立空间复用你的登录态，用于打开网页、填表、点击、截图、抓取、Web 测试等。附带 ego-browser Skill（由 Kivio 从仓库自动下载）。",
    binary: "ego-browser",
    tags: &["Browser", "Automation", "Skill", "macOS"],
    homepage: "https://lite.ego.app/",
    repo: "https://github.com/citrolabs/ego-lite",
    // onboarding 后注册到用户 bin；PATH 未刷新时按这些路径兜底检测。
    // macOS 无 %USERPROFILE% 环境变量，须用展开器支持的 $HOME。
    known_binary_paths: &[
        "$HOME/.local/bin/ego-browser",
        "/usr/local/bin/ego-browser",
        "/opt/homebrew/bin/ego-browser",
    ],
    readme_urls: &["https://raw.githubusercontent.com/citrolabs/ego-lite/main/README.md"],
    system_hint: "\
### ego lite (plugin: ego-lite)\n\
**Role.** Real Chromium browser automation for interactive web tasks — open pages, fill forms, click, screenshot, scrape, log in, test web apps.\n\
Prefer the `ego-browser` skill over web_fetch / built-in browsing whenever the task needs real page interaction.\n\
Activate the `ego-browser` skill, then run browser work via run_command as `ego-browser nodejs <<'EOF' … EOF` (default one invocation per task). Do NOT import Playwright or launch another browser.",
    skill_ids: &["ego-browser"],
    skill_md: "", // 由 Kivio 从 skill_download_url 下载
    skill_download_url: Some("https://github.com/citrolabs/ego-lite"),
    mcp: None,
    install_doc: EGO_LITE_INSTALL_DOC,
}];

/// `officecli load_skill` / skills install 的完整集合（CLI 子名 → frontmatter skill id）。
/// base `officecli` 单独从 skills install 目录复制，不在此表。
pub const OFFICECLI_DOMAIN_SKILLS: &[(&str, &str)] = &[
    ("pptx", "officecli-pptx"),
    ("word", "officecli-docx"),
    ("excel", "officecli-xlsx"),
    ("morph-ppt", "morph-ppt"),
    ("morph-ppt-3d", "morph-ppt-3d"),
    ("pitch-deck", "officecli-pitch-deck"),
    ("academic-paper", "officecli-academic-paper"),
    ("data-dashboard", "officecli-data-dashboard"),
    ("financial-model", "officecli-financial-model"),
    ("word-form", "officecli-word-form"),
];

/// 插件门闸 / UI 展示用的 skill id 列表（含 base）。
pub const OFFICECLI_OFFICIAL_SKILL_IDS: &[&str] = &[
    "officecli",
    "officecli-pptx",
    "officecli-docx",
    "officecli-xlsx",
    "morph-ppt",
    "morph-ppt-3d",
    "officecli-pitch-deck",
    "officecli-academic-paper",
    "officecli-data-dashboard",
    "officecli-financial-model",
    "officecli-word-form",
];

pub fn catalog_plugin(id: &str) -> Option<&'static CatalogPlugin> {
    PLUGIN_CATALOG.iter().find(|p| p.id == id)
}

/// ego lite 安装补充：GUI 应用（macOS .dmg），Skill 由 Kivio 下载，无 MCP、无 brew。
const EGO_LITE_INSTALL_DOC: &str = r#"## 本插件补充（ego lite）

- **仅 macOS**。ego lite 是 GUI 应用（.dmg）；`ego-browser` 命令由 app 完成首次 onboarding 后注册到 PATH（通常 `~/.local/bin`）。
- **安装 app（二选一）**：
  1. 官网 / README 里的直链下载 .dmg：https://lite.ego.app/ ，下载后打开 .dmg 安装；
  2. 或运行技能自带脚本（若在本机）：`sh skills/ego-browser/scripts/install.sh`（macOS only）。
- 安装后**请用户在 app 内完成一次 onboarding**（可选导入 Chrome 数据），onboarding 会注册 `ego-browser` 到 `~/.local/bin`。等用户确认完成再继续。
- 验证：`command -v ego-browser`（找不到则 `export PATH="$HOME/.local/bin:$PATH"` 重试），再跑：
  ```
  ego-browser nodejs <<'EOF'
  console.log('ego-browser ready')
  EOF
  ```
- **Skill 由 Kivio 负责**：用户点「启用」后，Kivio 自动从仓库下载 `ego-browser` Skill 并接入对话——**你（AI）不要手写 Skill，也不要配置任何 MCP（本插件无 MCP）**。
- **无 brew cask**：不要 `brew install`。
"#;

/// 插件专属补充。通用「读 GitHub / 兼容 Kivio」写在 get_install_brief 模板里。
const OFFICECLI_INSTALL_DOC: &str = r#"## 本插件补充（OfficeCLI）

| 字段 | 值 |
|------|-----|
| plugin_id | officecli |
| 命令名 | officecli |
| 官网 | https://officecli.ai |
| 常见 Windows 安装目录 | `%LOCALAPPDATA%\OfficeCLI\officecli.exe` |

### 安装阶段（本对话 · 由 Kivio AI 执行，非后台脚本）

1. 按 README 安装 **官方** `officecli` 二进制；验收 `officecli --version`。
2. **官方 Skills — 必须全量安装**（不要只装 pptx/word/excel，不要用自写 stub）：
   - `officecli skills install`（base）
   - 对 **每一个** 领域 skill 执行 `officecli skills install <name>`：
     `pptx` `word` `excel` `morph-ppt` `morph-ppt-3d` `pitch-deck` `academic-paper` `data-dashboard` `financial-model` `word-form`
   - 可用循环一次性装完（PowerShell 示例）：
     ```
     foreach ($s in 'pptx','word','excel','morph-ppt','morph-ppt-3d','pitch-deck','academic-paper','data-dashboard','financial-model','word-form') { officecli skills install $s }
     ```
   - 验收：`officecli skills list` 中上述 skill **全部**为 installed（或等价成功）；抽查 `load_skill pptx`、`load_skill pitch-deck` 能打出正文。
3. **不要**执行 `officecli mcp claude|cursor|vscode|…`（那是给其它 IDE 的）。

### 启用阶段（用户拨开关 · Kivio 运行时自动）

1. **MCP**：注册官方 stdio `plugin-officecli` = `{绝对路径} mcp`（官方内置 MCP）。
2. **Skill**：把**全部**官方 skill 接入 Kivio 对话，供 `skill` 激活。
3. **系统提示**：Kivio 适配策略（优先 MCP、禁止 watch 等）。

装完二进制和 Skills 后务必提醒用户去插件页 **启用**，否则 MCP/Skill 不会进对话。
"#;
