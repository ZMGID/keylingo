//! 内置插件目录：广场条目 + 安装规范 + 启用后注入的 MCP / 提示。
//!
//! CLI 插件（OfficeCLI、Cua Driver）用**官方安装器**；Skill 落在 `~/.agents/skills`，
//! Kivio 直接扫描。「让 AI 代装」只是可选。ego lite 仍在启用时从仓库下载 Skill。

/// 官方 README 里的安装命令（按平台）。只给后端自动执行，不展示给用户。
#[derive(Debug, Clone)]
pub struct PluginInstallCommand {
    /// windows | unix | macos | linux | any
    pub platform: &'static str,
    pub command: &'static str,
}

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
    /// 官方 README 原文 URL（raw）；权威安装/用法以 README 为准
    pub readme_urls: &'static [&'static str],
    /// GitHub README 里的官方安装命令（按平台）
    pub install_commands: &'static [PluginInstallCommand],
    /// 短 system 提示（仅 enabled 时注入）
    pub system_hint: &'static str,
    /// 本插件拥有的 skill id 列表（启用才可见）
    pub skill_ids: &'static [&'static str],
    /// 附属 Skill 正文（启用时写入插件目录；官方共享目录型插件保持为空）
    pub skill_md: &'static str,
    /// 可选：从该 GitHub 仓库 / 直链 zip 下载附属 Skill（启用时下载进插件 skill 目录，随上游更新）。
    /// 与 `skill_md` 二选一；两者皆空时走官方 `~/.agents/skills`（见 [`CatalogPlugin::uses_shared_skill_dirs`]）。
    pub skill_download_url: Option<&'static str>,
    /// 可选 MCP：启用时自动注册 stdio server
    pub mcp: Option<PluginMcpSpec>,
    /// **Kivio 安装契约**（薄层）：流程/约束/验收；具体命令以 README 为准，勿与 README 冲突
    pub install_doc: &'static str,
}

impl CatalogPlugin {
    /// 官方安装器把 Skill 写到 `~/.agents/skills` 等共享目录；Kivio 直接扫描，启用时不必再拷贝。
    pub fn uses_shared_skill_dirs(&self) -> bool {
        !self.skill_ids.is_empty()
            && self.skill_md.trim().is_empty()
            && self.skill_download_url.is_none()
    }

    /// 当前系统对应的 GitHub README 安装命令。
    pub fn host_install_command(&self) -> Option<&'static str> {
        let mut unix = None;
        for cmd in self.install_commands {
            match cmd.platform {
                "any" => return Some(cmd.command),
                "windows" if cfg!(windows) => return Some(cmd.command),
                "macos" if cfg!(target_os = "macos") => return Some(cmd.command),
                "linux" if cfg!(target_os = "linux") => return Some(cmd.command),
                "unix" if cfg!(unix) => unix = Some(cmd.command),
                _ => {}
            }
        }
        unix
    }
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
    install_commands: &[
        PluginInstallCommand {
            platform: "unix",
            command: "curl -fsSL https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh | bash",
        },
        PluginInstallCommand {
            platform: "windows",
            command: "irm https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.ps1 | iex",
        },
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
    skill_md: "", // 空 + 无 download = 官方安装器写入 ~/.agents/skills，Kivio 直接扫描
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
    install_commands: &[PluginInstallCommand {
        platform: "macos",
        command: "npx skills add citrolabs/ego-lite",
    }],
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
}, CatalogPlugin {
    id: "cua-driver",
    name: "Cua Driver",
    description: "面向 AI Agent 的后台桌面操控。在 macOS / Windows / Linux 上点击、输入、读取无障碍树与窗口截图，不抢鼠标焦点。附带 Skill 与 MCP。",
    binary: "cua-driver",
    tags: &["Desktop", "Computer Use", "CLI", "Skill", "MCP"],
    homepage: "https://cua.ai/cua-driver",
    repo: "https://github.com/trycua/cua",
    known_binary_paths: &[
        r"%LOCALAPPDATA%\Programs\Cua\cua-driver\bin\cua-driver.exe",
        r"%USERPROFILE%\.local\bin\cua-driver",
        "$HOME/.local/bin/cua-driver",
        "/usr/local/bin/cua-driver",
        "/opt/homebrew/bin/cua-driver",
    ],
    readme_urls: &[
        "https://raw.githubusercontent.com/trycua/cua/main/README.md",
        "https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/README.md",
    ],
    install_commands: &[
        PluginInstallCommand {
            platform: "unix",
            command: r#"/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)""#,
        },
        PluginInstallCommand {
            platform: "windows",
            command: "irm https://cua.ai/driver/install.ps1 | iex",
        },
    ],
    system_hint: "\
### Cua Driver (plugin: cua-driver)\n\
**Role.** Drive native desktop apps in the background — click, type, scroll, inspect accessibility trees, capture window screenshots — without stealing the user's cursor or focus.\n\
Prefer this plugin over ad-hoc GUI scripts (PyAutoGUI, osascript click storms, cliclick) whenever the task is operating a real app on the host.\n\
\n\
**Skill.** Activate `cua-driver` before substantial desktop-driving work. Follow its snapshot-before-action loop; do not call tools ad-hoc.\n\
\n\
**Do NOT:**\n\
- Run `cua-driver mcp-config --client claude|cursor|…` (Kivio already registers the official stdio server as plugin-cua-driver).\n\
- Run `pip install cua` for this plugin — that is the separate Sandbox SDK, not the desktop driver.\n\
- Launch a second computer-use driver when this plugin is enabled.\n\
\n\
**MCP first.** Prefer MCP tools (`list_apps`, `list_windows`, `get_window_state`, `click`, `type_text`, …) over `run_command cua-driver call …`. One persistent MCP process holds the runtime; bash cold-starts per call.\n\
\n\
**macOS.** `cua-driver mcp` proxies to CuaDriver.app. If tools fail on permissions, tell the user to grant Accessibility + Screen Recording to CuaDriver, then `open -n -g -a CuaDriver --args serve`.\n\
\n\
**Done.** Report the app/window driven and what you verified (screenshot / AX tree).",
    skill_ids: &["cua-driver"],
    skill_md: "", // 空 + 无 download = 官方 `skills install` 写入 ~/.agents 等，Kivio 直接扫描
    skill_download_url: None,
    mcp: Some(PluginMcpSpec { args: &["mcp"] }),
    install_doc: CUA_DRIVER_INSTALL_DOC,
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

1. 按 README 安装 **官方** `officecli` 二进制：
   ```
   # macOS / Linux
   curl -fsSL https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh | bash
   # Windows (PowerShell)
   irm https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.ps1 | iex
   ```
   验收 `officecli --version`。
2. **官方 Skills**：官方安装器 / `officecli skills install …` 会写入 `~/.agents/skills`（以及 `~/.claude/skills` 等）。Kivio 直接扫描这些目录，**不必**再拷进插件目录。
   若本机还没有，再执行 `officecli skills install`，以及领域包：`pptx` `word` `excel` `morph-ppt` `morph-ppt-3d` `pitch-deck` `academic-paper` `data-dashboard` `financial-model` `word-form`。
3. **不要**执行 `officecli mcp claude|cursor|vscode|…`（那是给其它 IDE 的）。

### 启用阶段（用户拨开关 · Kivio 运行时自动）

1. **MCP**：注册官方 stdio `plugin-officecli` = `{绝对路径} mcp`（官方内置 MCP）。
2. **Skill**：已在 `~/.agents/skills` 的官方 skill 直接进对话（插件启用后才放行）。
3. **系统提示**：Kivio 适配策略（优先 MCP、禁止 watch 等）。

装完官方二进制后提醒用户去插件页 **刷新并启用**，否则 MCP 不会进对话。
"#;

/// Cua Driver：官方一键安装器 + 单包 Skill + stdio MCP。不要和 `pip install cua`（Sandbox SDK）搞混。
const CUA_DRIVER_INSTALL_DOC: &str = r#"## 本插件补充（Cua Driver）

| 字段 | 值 |
|------|-----|
| plugin_id | cua-driver |
| 命令名 | cua-driver |
| 官网 | https://cua.ai/cua-driver |
| 仓库 | https://github.com/trycua/cua |
| 常见 Windows 安装目录 | `%LOCALAPPDATA%\Programs\Cua\cua-driver\bin\cua-driver.exe` |
| 常见 Unix 安装路径 | `~/.local/bin/cua-driver` |

**范围：** 本插件只要 **Cua Driver**（本机后台桌面操控）。不要安装 Cua Sandbox / Cua Bench / Lume，也不要 `pip install cua`。

### 安装阶段（本对话 · 由 Kivio AI 执行，非后台脚本）

1. 按 README 安装官方 **cua-driver** 二进制（以刚读到的 README 为准；常见一键脚本）：
   - macOS / Linux：`/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"`
   - Windows PowerShell：`irm https://cua.ai/driver/install.ps1 | iex`，然后 `cua-driver autostart kick`
2. 验收并贴出输出：
   ```
   cua-driver --version
   cua-driver doctor
   ```
3. **macOS 权限（必须等人）：** 先启动守护进程，再申请 TCC：
   ```
   open -n -g -a CuaDriver --args serve
   cua-driver permissions grant
   ```
   告诉用户：系统弹窗点「打开系统设置」后，在 **辅助功能** 和 **屏幕录制** 里打开 CuaDriver。完成后跑 `cua-driver permissions status`，两项都应为 granted。
4. **官方 Skill：** `cua-driver skills install --all-platforms` 写入 `~/.agents/skills` / `~/.cua-driver/skills`。Kivio 直接扫描，**不必**再拷进插件目录。
5. **不要**执行 `cua-driver mcp-config --client claude|cursor|vscode|…`（那是给其它 IDE 写配置的）。

### 启用阶段（用户拨开关 · Kivio 运行时自动）

1. **MCP**：注册官方 stdio `plugin-cua-driver` = `{绝对路径} mcp`。
2. **Skill**：已在 `~/.agents/skills` 的 `cua-driver` 直接进对话（插件启用后才放行）。
3. **系统提示**：Kivio 适配策略（优先 MCP、禁止 mcp-config）。

装完官方二进制后提醒用户去插件页 **刷新并启用**，否则 MCP 不会进对话。
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_pins_three_plugins() {
        assert_eq!(PLUGIN_CATALOG.len(), 3);
        assert!(catalog_plugin("officecli").is_some());
        assert!(catalog_plugin("ego-lite").is_some());
        assert!(catalog_plugin("cua-driver").is_some());
    }

    #[test]
    fn cua_driver_is_cli_mcp_skill_plugin() {
        let p = catalog_plugin("cua-driver").expect("cua-driver");
        assert_eq!(p.binary, "cua-driver");
        assert_eq!(p.skill_ids, &["cua-driver"]);
        let mcp = p.mcp.as_ref().expect("stdio mcp");
        assert_eq!(mcp.args, &["mcp"]);
        assert!(p.install_doc.contains("cua-driver skills install"));
        assert!(p.install_commands.iter().any(|c| {
            c.platform == "windows" && c.command.contains("cua.ai/driver/install.ps1")
        }));
        assert!(p.install_commands.iter().any(|c| {
            c.platform == "unix" && c.command.contains("cua.ai/driver/install.sh")
        }));
        let office = catalog_plugin("officecli").unwrap();
        assert!(office.install_commands.iter().any(|c| {
            c.command.contains("iOfficeAI/OfficeCLI/main/install.ps1")
        }));
        assert!(office.install_commands.iter().any(|c| {
            c.command.contains("iOfficeAI/OfficeCLI/main/install.sh")
        }));
        let ego = catalog_plugin("ego-lite").unwrap();
        assert!(ego
            .install_commands
            .iter()
            .any(|c| c.platform == "macos" && c.command.contains("npx skills add citrolabs/ego-lite")));
        assert!(p.uses_shared_skill_dirs());
        assert!(office.uses_shared_skill_dirs());
        assert!(!ego.uses_shared_skill_dirs());
        assert!(
            p.install_doc.contains("不要 `pip install cua`")
                || p.install_doc.contains("也不要 `pip install cua`")
        );
        assert!(p.system_hint.contains("plugin-cua-driver"));
        #[cfg(windows)]
        {
            assert_eq!(
                office.host_install_command(),
                Some("irm https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.ps1 | iex")
            );
            assert_eq!(
                p.host_install_command(),
                Some("irm https://cua.ai/driver/install.ps1 | iex")
            );
            assert_eq!(ego.host_install_command(), None);
        }
        #[cfg(unix)]
        {
            assert_eq!(
                office.host_install_command(),
                Some("curl -fsSL https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh | bash")
            );
        }
    }
}
