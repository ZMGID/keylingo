//! 插件状态列表、启用/卸载、以及运行官方 README 安装命令。
//!
//! 点「安装」会执行 catalog 里对应平台的 GitHub README 命令（允许名单，不是用户输入）。
//! Skill 已在 `~/.agents/skills` 时不必再拷贝。

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::catalog::{catalog_plugin, CatalogPlugin, OFFICECLI_DOMAIN_SKILLS, PLUGIN_CATALOG};
use super::lifecycle::{
    apply_disable_side_effects, apply_enable_side_effects, plugin_mcp_server_id,
};
use super::state::{
    default_binary_filename, is_enabled, kivio_binary_path, meta_path, plugin_dir, probe_version,
    read_meta, refresh_process_path_for_detection, resolve_binary, resolve_binary_for_status,
    skill_dir, write_meta, PluginMeta,
};
use crate::proc::NoConsoleWindow;
use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const OFFICIAL_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const SKILLS_INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const OFFICIAL_INSTALL_OUTPUT_CAP: usize = 8_000;

static OFFICIAL_SKILL_SYNC: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub binary: String,
    pub tags: Vec<String>,
    pub homepage: String,
    pub repo: String,
    pub installed: bool,
    /// 仅已安装时有意义；检测到二进制后默认仍为 false，需用户启用
    pub enabled: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    /// kivio | system | none
    pub source: String,
    pub has_skill: bool,
    pub has_mcp: bool,
    pub skill_ids: Vec<String>,
    /// 本插件配置的 Skill 数量（catalog）
    pub skill_count: u32,
    /// 本插件配置的 MCP 数量（catalog，通常 0 或 1）
    pub mcp_count: u32,
    /// 启用后：Skill 文件是否已就绪
    pub skill_active: bool,
    /// 启用后：settings 里是否已有插件 MCP 且 enabled
    pub mcp_active: bool,
    /// 启用后 MCP 的 server id（如 plugin-officecli）
    pub mcp_server_id: Option<String>,
    /// 当前系统是否有可自动执行的官方安装器（命令本身不回传前端）
    pub can_install: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginActionResult {
    pub ok: bool,
    pub message: String,
    pub status: PluginStatus,
}

/// 交给聊天 Agent 的安装任务包。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallBrief {
    pub plugin_id: String,
    pub plugin_name: String,
    /// 新对话标题
    pub conversation_title: String,
    /// 官方 README raw URL（安装时须先 fetch）
    pub readme_urls: Vec<String>,
    /// 作为用户消息发给 Agent 的完整正文（含 README 要求 + 安装契约）
    pub user_message: String,
}

pub fn list_plugin_statuses() -> Result<Vec<PluginStatus>, String> {
    // AI 安装后可能刚写进用户 Path / 默认安装目录；每次列表都重新探测
    refresh_process_path_for_detection();
    Ok(PLUGIN_CATALOG.iter().map(status_for).collect())
}

/// 无 spawn 的缓存态列表：只查文件（kivio 托管路径）+ meta.json（enabled/version），
/// 不跑 `which` / `--version` 子进程。前端首屏用它秒开页面；完整探测只在手动刷新/操作后跑。
pub fn list_plugin_statuses_cached() -> Result<Vec<PluginStatus>, String> {
    Ok(PLUGIN_CATALOG.iter().map(status_for_cached).collect())
}

/// 填充 mcp_active（settings 里该 server 是否已注册且 enabled）。纯 settings 读，无 spawn。
fn fill_mcp_active(list: &mut [PluginStatus], state: &AppState) {
    let settings = state.settings_read();
    for status in list.iter_mut() {
        if let Some(sid) = status.mcp_server_id.as_deref() {
            status.mcp_active = settings
                .chat_tools
                .servers
                .iter()
                .any(|s| s.id == sid && s.enabled && !s.command.trim().is_empty());
        }
    }
}

/// 构造一条 PluginStatus。`installed` 由调用方决定（精确探测用 `path.is_some()`，缓存态用
/// kivio 路径存在 + meta 记录），`path`/`version` 同理由调用方按快/慢路径填入。
fn build_status(
    catalog: &CatalogPlugin,
    kivio: &Option<PathBuf>,
    path: Option<PathBuf>,
    installed: bool,
    version: Option<String>,
) -> PluginStatus {
    let source = if kivio.is_some() {
        "kivio".to_string()
    } else if installed {
        "system".to_string()
    } else {
        "none".to_string()
    };
    let enabled = is_enabled(catalog.id) && installed;
    let skill_count = catalog.skill_ids.len() as u32;
    let mcp_count = if catalog.mcp.is_some() { 1 } else { 0 };
    // 有 skill_ids 即声明附属 skill（插件目录、或官方 ~/.agents 等共享目录）
    let has_skill = skill_count > 0;
    let has_mcp = mcp_count > 0;
    let skill_active = enabled && has_skill && plugin_has_any_official_or_copied_skill(catalog);
    // MCP 是否已挂到 settings 由 fill_mcp_active 再补
    let mcp_server_id = has_mcp.then(|| plugin_mcp_server_id(catalog.id));
    PluginStatus {
        id: catalog.id.to_string(),
        name: catalog.name.to_string(),
        description: catalog.description.to_string(),
        binary: catalog.binary.to_string(),
        tags: catalog.tags.iter().map(|s| (*s).to_string()).collect(),
        homepage: catalog.homepage.to_string(),
        repo: catalog.repo.to_string(),
        installed,
        enabled,
        version,
        path: path.map(|p| p.display().to_string()),
        source,
        has_skill,
        has_mcp,
        skill_ids: catalog.skill_ids.iter().map(|s| (*s).to_string()).collect(),
        skill_count,
        mcp_count,
        skill_active,
        mcp_active: false,
        mcp_server_id,
        can_install: catalog.host_install_command().is_some(),
    }
}

fn status_for(catalog: &CatalogPlugin) -> PluginStatus {
    let kivio = kivio_binary_path(catalog.id);
    // 此处不再重复 enrich（list 已做）；单条 status 也走完整 resolve（含 which spawn）。
    let path = resolve_binary(catalog.id);
    let installed = path.is_some();
    let version = path
        .as_ref()
        .and_then(|p| probe_version(p))
        .or_else(|| read_meta(catalog.id).and_then(|m| m.version));
    build_status(catalog, &kivio, path, installed, version)
}

/// 缓存态：无子进程。installed 取「kivio 托管二进制存在」或「meta 记录装过/启用过」，
/// version 直接用 meta 缓存值。system-PATH 安装但无 meta 的插件此处可能暂显未装，随后
/// 被手动刷新的完整探测修正——秒开优先，短暂不精确可接受。
fn status_for_cached(catalog: &CatalogPlugin) -> PluginStatus {
    let kivio = kivio_binary_path(catalog.id);
    let meta = read_meta(catalog.id);
    let installed = kivio.is_some()
        || meta
            .as_ref()
            .map(|m| m.enabled || m.version.is_some() || m.installed_at.is_some())
            .unwrap_or(false);
    let version = meta.and_then(|m| m.version);
    build_status(catalog, &kivio, kivio.clone(), installed, version)
}

/// 用 AppState 填充 mcp_active（settings 里是否已注册且 enabled）
pub fn list_plugin_statuses_with_state(state: &AppState) -> Result<Vec<PluginStatus>, String> {
    refresh_process_path_for_detection();
    let mut list: Vec<PluginStatus> = PLUGIN_CATALOG.iter().map(status_for).collect();
    fill_mcp_active(&mut list, state);
    spawn_missing_official_skills(&list);
    Ok(list)
}

/// 缓存态 + mcp_active（无探测子进程）。已启用但缺 Skill 的插件会后台补装。
pub fn list_plugin_statuses_cached_with_state(
    state: &AppState,
) -> Result<Vec<PluginStatus>, String> {
    let mut list = list_plugin_statuses_cached()?;
    fill_mcp_active(&mut list, state);
    spawn_missing_official_skills(&list);
    Ok(list)
}

/// 生成「让 Kivio AI 安装」的用户消息 + 标题。
///
/// 安装**全程由本对话里的 Kivio AI 操作**（run_command / web_fetch），**不是** Kivio
/// 后端静默脚本下载。AI 负责：读 README → 装二进制 → 装官方 Skill 包 → 验收。
/// 用户点「启用」后，Kivio 运行时再挂官方 MCP stdio + 把已装官方 Skill 接入 Agent。
pub fn get_install_brief(id: &str) -> Result<PluginInstallBrief, String> {
    let catalog = catalog_plugin(id).ok_or_else(|| format!("unknown plugin: {id}"))?;
    // GUI 应用型插件（Skill 由 Kivio 下载、无 MCP，如 ego lite）：officecli 那套 MCP/skills-install
    // 模板不适用，走精简简报。
    if catalog.skill_download_url.is_some() {
        return Ok(build_downloaded_skill_app_brief(catalog));
    }
    let readme_urls: Vec<String> = catalog
        .readme_urls
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let readme_block = if readme_urls.is_empty() {
        format!(
            "- （未配置 raw URL）请打开仓库页面阅读 README：{}",
            catalog.repo
        )
    } else {
        readme_urls
            .iter()
            .map(|u| format!("- {u}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let user_message = format!(
        r#"# 安装 Kivio 插件：{name}

## 1. 项目地址

- **GitHub：** {repo}
- **官网：** {homepage}
- **plugin_id：** `{id}`
- **CLI 命令名：** `{binary}`

## 2. 强制：先通读官方 README，再动手

**任何安装命令之前必须完成：**

1. `web_fetch` 打开仓库 {repo}。
2. `web_fetch` 拉取完整 README（按序尝试，优先中文）：
{readme_block}
3. **通读** README（安装 / PATH / Skills / MCP 相关都要读）。
4. 用 3～6 句话向用户复述：用途 + 官方推荐安装方式 + 你将执行的步骤。
5. **然后才**安装。命令必须来自刚读到的 README（或其指向的官方脚本/Release），禁止凭记忆编造。

README 失败：说明原因并给 {repo} / Releases，不要瞎装。

## 3. 分工（必读）

Kivio **不会**用后台脚本静默下载插件。本任务由 **你（Kivio AI）** 在本对话里用工具完成。

| 阶段 | 谁做 | 你要做什么 |
|------|------|------------|
| **A. 安装（本对话）** | **你（AI）** | ① 按 README 安装官方 `{binary}` ② 若 `~/.agents/skills` 已有官方 Skill 则跳过，否则补 `skills install` ③ 验收并汇报 |
| **B. 检测** | 用户点插件页「刷新」 | 你只需保证 PATH/默认目录里能跑 `{binary}` |
| **C. 启用** | **用户**打开插件开关 | **不要**手改 Kivio settings；启用后 Kivio **自动**挂上官方 MCP（`… mcp` stdio）并把官方 Skill 接入对话 |
| **D. 关闭** | 用户 | 卸下 MCP / Skill / 系统提示 |

### 3.1 MCP：官方能力，Kivio 启用时自动接线

- 官方 MCP = 本机 `{binary} mcp`（stdio JSON-RPC），**不是** Kivio 自研协议。
- README 里的 `{binary} mcp claude|cursor|vscode|…` 是给**其它 IDE**写配置的。在 Kivio 里：
  - **禁止**执行 `mcp claude` / `cursor` / `vscode` / `lmstudio` 等。
  - **禁止**手改 Kivio 的 MCP 列表 / settings.json。
  - 用户 **启用** 插件后，Kivio 会注册：`command=<绝对路径>`，`args=["mcp"]`（id 形如 `plugin-{id}`）。
- 你在安装阶段**不要**声称「MCP 已在 Kivio 里可用」——那要等用户启用。

### 3.2 Skill：官方安装器写入 `~/.agents/skills`，Kivio 直接扫描

官网安装器 / `{binary} skills install` 会把 Skill 写到 `~/.agents/skills`（以及 `~/.claude/skills` 等）。Kivio **直接扫描这些目录**，不要再拷进插件目录、不要手写 stub。

若本机已经有对应 SKILL.md：**跳过** skills install。只有缺文件时才按 **§5** 补装。

- **禁止**自己编写「精简 SKILL.md」代替官方内容。
- **禁止**用其它库替代本插件。
- 用户启用插件后，这些官方 skill 才对 Agent 放行。

## 4. 安装步骤清单（按序执行）

### 4.1 二进制

1. 按 README 官方方式安装（一键脚本 / brew / scoop / Release 等，以 README 为准）。
2. 验收并贴出输出：
   ```
   {binary} --version
   ```
   尽量给出可执行文件完整路径。
3. 若已安装且 version 正常：报告版本与路径，**不要重复安装**（除非用户要求升级）。

### 4.2 官方 Skills

见 §5。若 `~/.agents/skills` 里已有官方 SKILL.md 则跳过；否则按该节补装。

### 4.3 收尾对用户说

装完后明确告知用户（原话意思即可）：

> 二进制与官方 Skills 已装好。请到 Kivio → **扩展 → 插件**，找到 **{name}**，打开 **启用**。  
> 启用后 Kivio 会自动挂载官方 MCP（`{binary} mcp`）并接入官方 Skills；无需再配 Claude/Cursor 的 mcp 命令。

PATH 若仅新终端生效：请用户在插件页点刷新，或重启 Kivio。优先用户级安装，非必要不要管理员/sudo。

失败时：说明卡在哪一步 → 按 README 换官方方式 → 仍失败则给仓库与 Releases 链接。

## 5. 本插件补充

{doc}
"#,
        name = catalog.name,
        repo = catalog.repo,
        homepage = catalog.homepage,
        id = catalog.id,
        binary = catalog.binary,
        readme_block = readme_block,
        doc = catalog.install_doc.trim(),
    );
    Ok(PluginInstallBrief {
        plugin_id: catalog.id.to_string(),
        plugin_name: catalog.name.to_string(),
        conversation_title: format!("安装插件 · {}", catalog.name),
        readme_urls,
        user_message,
    })
}

fn official_install_program_args(script: &str) -> (&'static str, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                script.to_string(),
            ],
        )
    }
    #[cfg(not(windows))]
    {
        ("bash", vec!["-lc".into(), script.to_string()])
    }
}

fn trim_install_output(stdout: &[u8], stderr: &[u8]) -> String {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let mut parts = Vec::new();
    if !out.trim().is_empty() {
        parts.push(out.trim().to_string());
    }
    if !err.trim().is_empty() {
        parts.push(err.trim().to_string());
    }
    let mut text = parts.join("\n");
    if text.len() > OFFICIAL_INSTALL_OUTPUT_CAP {
        let start = text.len() - OFFICIAL_INSTALL_OUTPUT_CAP;
        text = format!("…{}", &text[start..]);
    }
    text
}

/// 运行 catalog 里当前系统的 GitHub README 安装命令（允许名单）。
pub async fn run_official_install(id: &str) -> Result<PluginActionResult, String> {
    let catalog = catalog_plugin(id).ok_or_else(|| format!("unknown plugin: {id}"))?;
    let script = catalog
        .host_install_command()
        .ok_or_else(|| "当前系统暂不支持自动安装该插件".to_string())?;
    if !catalog.install_commands.iter().any(|cmd| cmd.command == script) {
        return Err("安装失败，请稍后重试。".to_string());
    }

    let (program, args) = official_install_program_args(script);
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .no_console_window();

    let child = cmd
        .spawn()
        .map_err(|_| "无法开始安装".to_string())?;
    let output = tokio::time::timeout(OFFICIAL_INSTALL_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "安装超时（10 分钟）。可再点一次安装。".to_string())?
        .map_err(|_| "安装失败，请稍后重试。".to_string())?;

    let detail = trim_install_output(&output.stdout, &output.stderr);
    if !detail.is_empty() {
        eprintln!("[plugins] official install {}:\n{detail}", catalog.id);
    }
    refresh_process_path_for_detection();

    if !output.status.success() {
        return Err("安装失败，请稍后重试。".to_string());
    }

    spawn_official_skill_sync(id);

    let status = status_for(catalog);
    let message = if status.installed {
        format!("已安装 {}。打开启用后即可使用。", catalog.name)
    } else {
        format!(
            "安装已结束，但尚未检测到 {}。请点刷新后再试。",
            catalog.name
        )
    };
    Ok(PluginActionResult {
        ok: true,
        message,
        status,
    })
}

/// 面向「GUI 应用 + Kivio 下载 Skill」型插件（如 ego lite）的安装简报：
/// AI 只负责装 app + 引导 onboarding；Skill 由 Kivio 启用时下载，本插件无 MCP。
fn build_downloaded_skill_app_brief(catalog: &CatalogPlugin) -> PluginInstallBrief {
    let readme_urls: Vec<String> = catalog
        .readme_urls
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let readme_block = if readme_urls.is_empty() {
        format!("- 打开仓库阅读 README：{}", catalog.repo)
    } else {
        readme_urls
            .iter()
            .map(|u| format!("- {u}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let user_message = format!(
        r#"# 安装 Kivio 插件：{name}

## 1. 项目

- **GitHub：** {repo}
- **官网：** {homepage}
- **plugin_id：** `{id}`
- **命令名：** `{binary}`

## 2. 先读 README，再动手

1. `web_fetch` 拉取 README（安装 / onboarding 部分都要读）：
{readme_block}
2. 用 2～4 句向用户复述用途 + 官方安装方式 + 你将执行的步骤。命令须来自 README，勿凭记忆编造。

## 3. 分工（必读）

- **你（AI）**：按官方方式安装应用，并引导用户完成首次 onboarding，直到 `{binary}` 命令可用。
- **Skill：不用你装。** 用户在「扩展 → 插件」点**启用**后，Kivio 会自动从仓库下载 `{binary}` 的官方 Skill 并接入对话。**禁止**手写 / 精简 Skill。
- **MCP：本插件无 MCP。** 不要配置任何 mcp 命令，也不要改 Kivio settings。

## 4. 安装步骤

{doc}

## 5. 收尾对用户说

> 应用与 onboarding 完成后，请到 Kivio →「扩展 → 插件」，找到 **{name}** 点**刷新**确认已检测到，再打开**启用**。启用后 Kivio 会自动下载并接入官方 Skill。
"#,
        name = catalog.name,
        repo = catalog.repo,
        homepage = catalog.homepage,
        id = catalog.id,
        binary = catalog.binary,
        readme_block = readme_block,
        doc = catalog.install_doc.trim(),
    );
    PluginInstallBrief {
        plugin_id: catalog.id.to_string(),
        plugin_name: catalog.name.to_string(),
        conversation_title: format!("安装插件 · {}", catalog.name),
        readme_urls,
        user_message,
    }
}

/// 启用「下载型」插件时，从 `skill_download_url` 拉取 Skill 到 `plugins/<id>/skills/`
/// （解压 zip 内首个 SKILL.md 文件夹到 `.../skills/<skill_id>/`）。
async fn download_plugin_skill(catalog: &CatalogPlugin, url: &str) -> Result<(), String> {
    let dest = plugin_dir(catalog.id)
        .ok_or_else(|| "app data directory unavailable".to_string())?
        .join("skills");
    std::fs::create_dir_all(&dest).map_err(|e| format!("create plugin skills dir: {e}"))?;
    crate::skills::download_skill_zip_into(url, &dest).await?;
    Ok(())
}

pub async fn set_plugin_enabled(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    enabled: bool,
) -> Result<PluginActionResult, String> {
    let catalog = catalog_plugin(id).ok_or_else(|| format!("unknown plugin: {id}"))?;
    // 启用前强制再探测（含官方默认安装目录 + 刷新 PATH）
    let resolved = resolve_binary_for_status(id);
    if enabled && resolved.is_none() {
        return Err("尚未检测到插件。请先点「安装」，完成后再启用。".to_string());
    }

    let mut meta = read_meta(id).unwrap_or_else(|| PluginMeta {
        id: id.to_string(),
        version: resolved.as_ref().and_then(|p| probe_version(p)),
        enabled: false,
        installed_at: None,
        binary_name: default_binary_filename(id),
    });
    meta.enabled = enabled;
    if meta.id.is_empty() {
        meta.id = id.to_string();
    }
    // 系统 PATH / 官方目录安装也记一笔 meta，方便开关持久化
    if meta.installed_at.is_none() && enabled {
        meta.installed_at = Some(chrono::Utc::now().to_rfc3339());
    }
    if meta.version.is_none() {
        meta.version = resolved.as_ref().and_then(|p| probe_version(p));
    }
    write_meta(&meta)?;

    if enabled {
        // 下载型插件（如 ego lite）：从仓库拉 Skill 到插件目录；失败不阻断启用。
        if let Some(url) = catalog.skill_download_url {
            if let Err(err) = download_plugin_skill(catalog, url).await {
                eprintln!("[plugins] skill download on enable {id}: {err}");
            }
        } else if !catalog.uses_shared_skill_dirs() {
            if let Err(err) = write_skill_files(catalog) {
                eprintln!("[plugins] skill sync on enable {id}: {err}");
            }
        }
        // 先挂 MCP / PATH / 提示，再后台补官方 Skill。`skills install` 可能要下包，不能挡启用。
        apply_enable_side_effects(app, state, id)?;
        if catalog.mcp.is_some() {
            spawn_plugin_mcp_warmup(app, plugin_mcp_server_id(id));
        }
        if catalog.uses_shared_skill_dirs() {
            spawn_official_skill_sync(id);
        }
    } else {
        apply_disable_side_effects(app, state, id, false).await?;
    }

    let mut status = status_for(catalog);
    if let Some(sid) = status.mcp_server_id.as_deref() {
        let settings = state.settings_read();
        status.mcp_active = settings
            .chat_tools
            .servers
            .iter()
            .any(|s| s.id == sid && s.enabled);
    }
    status.skill_active =
        enabled && status.has_skill && plugin_has_any_official_or_copied_skill(catalog);

    let message = if enabled {
        let mut parts = vec![format!("已启用 {}", catalog.name)];
        if status.skill_active {
            parts.push("官方 Skill 已接入".to_string());
        } else if status.has_skill {
            parts.push("官方 Skill 正在接入，稍后刷新即可".to_string());
        }
        if status.has_mcp {
            if status.mcp_active {
                parts.push("官方 MCP 已注册".to_string());
            } else {
                parts.push("MCP 注册失败，请重试启用".to_string());
            }
        }
        parts.push("新开对话或下一轮即可使用".to_string());
        parts.join("。")
    } else {
        format!("已关闭 {}（Skill / MCP / 系统提示均已卸下）", catalog.name)
    };

    Ok(PluginActionResult {
        ok: true,
        message,
        status,
    })
}

pub async fn uninstall_plugin(
    app: &AppHandle,
    state: &AppState,
    id: &str,
) -> Result<PluginActionResult, String> {
    let catalog = catalog_plugin(id).ok_or_else(|| format!("unknown plugin: {id}"))?;

    // 卸载前解析二进制；先停预览 / 杀进程，避免 Windows 删不掉 exe
    refresh_process_path_for_detection();
    let resolved = resolve_binary(id);
    if id == "officecli" {
        crate::plugins::stop_all_previews();
        kill_named_processes("officecli");
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    if id == "cua-driver" {
        if let Some(bin) = resolved.as_ref() {
            let _ = Command::new(bin)
                .args(["autostart", "disable"])
                .no_console_window()
                .output();
        }
        kill_named_processes("cua-driver");
        #[cfg(target_os = "macos")]
        kill_named_processes("CuaDriver");
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    apply_disable_side_effects(app, state, id, true).await?;

    let mut cleaned: Vec<String> = Vec::new();

    // 1) Kivio 插件数据（meta / 同步的 skill 缓存 / 预览 html）
    if let Some(dir) = plugin_dir(id) {
        if dir.is_dir() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => cleaned.push(format!("Kivio 插件目录 {}", dir.display())),
                Err(e) => cleaned.push(format!("Kivio 插件目录删除失败: {e}")),
            }
        } else if let Some(meta) = meta_path(id) {
            let _ = std::fs::remove_file(meta);
        }
    }

    // 2) 本机二进制与官方安装目录（干干净净，不只卸 Kivio 侧）
    if let Some(bin) = resolved.as_ref() {
        cleaned.extend(remove_cli_install(catalog, bin));
    }
    cleaned.extend(remove_known_binary_locations(catalog));

    // 3) 官方配置 / 写入各 Agent 的 skills / 安装器目录
    if id == "officecli" {
        cleaned.extend(remove_officecli_residuals());
    }
    if id == "cua-driver" {
        cleaned.extend(remove_cua_driver_residuals());
    }

    refresh_process_path_for_detection();
    let status = status_for(catalog);
    let detail = if cleaned.is_empty() {
        "未找到可删除的安装文件（可能已卸载）".to_string()
    } else {
        format!("已清理：{}", cleaned.join("；"))
    };
    Ok(PluginActionResult {
        ok: true,
        message: format!("已从本机卸载 {}。{detail}", catalog.name),
        status,
    })
}

/// 删除 CLI 可执行文件；若父目录是官方安装目录则整目录删除。
fn remove_cli_install(catalog: &CatalogPlugin, binary: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if !binary.is_file() {
        return out;
    }
    if is_protected_system_path(binary) {
        out.push(format!("跳过系统保护路径 {}", binary.display()));
        return out;
    }

    let parent = binary.parent().map(|p| p.to_path_buf());
    // 官方 Windows 目录 …\OfficeCLI\officecli.exe → 删整个 OfficeCLI 文件夹
    let remove_parent = parent.as_ref().is_some_and(|p| {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        name.eq_ignore_ascii_case("OfficeCLI")
            || name.eq_ignore_ascii_case(catalog.binary)
            || name.eq_ignore_ascii_case(&format!("{}-bin", catalog.binary))
    });

    if remove_parent {
        if let Some(p) = parent {
            match std::fs::remove_dir_all(&p) {
                Ok(()) => out.push(format!("安装目录 {}", p.display())),
                Err(e) => {
                    // 回退只删 exe
                    let _ = std::fs::remove_file(binary);
                    out.push(format!(
                        "安装目录删除失败({e})，已尝试删除 {}",
                        binary.display()
                    ));
                }
            }
            return out;
        }
    }

    match std::fs::remove_file(binary) {
        Ok(()) => out.push(format!("可执行文件 {}", binary.display())),
        Err(e) => out.push(format!("删除 {} 失败: {e}", binary.display())),
    }
    out
}

/// 按 catalog 已知路径再扫一遍，避免 resolve 只命中其中一处。
fn remove_known_binary_locations(catalog: &CatalogPlugin) -> Vec<String> {
    let mut out = Vec::new();
    for template in catalog.known_binary_paths {
        let expanded = expand_env_template(template);
        let path = PathBuf::from(&expanded);
        if !path.exists() {
            continue;
        }
        if path.is_file() {
            out.extend(remove_cli_install(catalog, &path));
        } else if path.is_dir() {
            // 模板若指向目录（少见）
            if let Err(e) = std::fs::remove_dir_all(&path) {
                out.push(format!("删除 {} 失败: {e}", path.display()));
            } else {
                out.push(format!("目录 {}", path.display()));
            }
        }
    }
    out
}

fn expand_env_template(template: &str) -> String {
    let mut s = template.to_string();
    for (key, val) in [
        (
            "%LOCALAPPDATA%",
            std::env::var("LOCALAPPDATA").unwrap_or_default(),
        ),
        (
            "%USERPROFILE%",
            std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_default(),
        ),
        ("%HOME%", std::env::var("HOME").unwrap_or_default()),
        ("%APPDATA%", std::env::var("APPDATA").unwrap_or_default()),
    ] {
        s = s.replace(key, &val);
    }
    s
}

fn is_protected_system_path(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy().to_ascii_lowercase();
    s.contains(r"\windows\system32")
        || s.contains(r"\windows\syswow64")
        || s.contains("/usr/bin/")
        || s.contains("/bin/sh")
        || s.ends_with(r"\windows")
}

/// OfficeCLI 配置与写入各 Agent 的 skills（officecli / officecli-pptx …）
fn remove_officecli_residuals() -> Vec<String> {
    let mut out = Vec::new();
    let home = match std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Some(h) => PathBuf::from(h),
        None => return out,
    };

    let config_dir = home.join(".officecli");
    if config_dir.is_dir() {
        match std::fs::remove_dir_all(&config_dir) {
            Ok(()) => out.push(format!("配置 {}", config_dir.display())),
            Err(e) => out.push(format!("配置删除失败: {e}")),
        }
    }

    let skill_roots = [
        home.join(".agents").join("skills"),
        home.join(".kivio").join("skills"),
        home.join(".claude").join("skills"),
        home.join(".cursor").join("skills"),
        home.join(".copilot").join("skills"),
        home.join(".codex").join("skills"),
        home.join(".hermes").join("skills"),
        home.join(".openclaw").join("skills"),
    ];
    for root in skill_roots {
        if !root.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for ent in entries.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            // 官方 skill 目录：officecli* 以及 morph-ppt / morph-ppt-3d（无 officecli- 前缀）
            let is_office_skill = name == "officecli"
                || name.starts_with("officecli-")
                || name == "morph-ppt"
                || name == "morph-ppt-3d";
            if is_office_skill {
                let p = ent.path();
                if p.is_dir() {
                    match std::fs::remove_dir_all(&p) {
                        Ok(()) => out.push(format!("Skill {}", p.display())),
                        Err(e) => out.push(format!("Skill {} 删除失败: {e}", p.display())),
                    }
                }
            }
        }
    }

    // 安装器目录（即使 resolve 没指到这里）
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let office = PathBuf::from(local).join("OfficeCLI");
        if office.is_dir() {
            match std::fs::remove_dir_all(&office) {
                Ok(()) => out.push(format!("安装目录 {}", office.display())),
                Err(e) => out.push(format!("安装目录 {} 删除失败: {e}", office.display())),
            }
        }
    }

    out
}

fn remove_named_dir(path: PathBuf, label: &str, out: &mut Vec<String>) {
    if !path.exists() {
        return;
    }
    match if path.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    } {
        Ok(()) => out.push(format!("{label} {}", path.display())),
        Err(e) => out.push(format!("{label} {} 删除失败: {e}", path.display())),
    }
}

/// Cua Driver 配置、官方 skill 链接、Windows 安装器目录、macOS app bundle。
fn remove_cua_driver_residuals() -> Vec<String> {
    let mut out = Vec::new();
    let home = match std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Some(h) => PathBuf::from(h),
        None => return out,
    };

    remove_named_dir(home.join(".cua-driver"), "配置", &mut out);

    let skill_roots = [
        home.join(".agents").join("skills"),
        home.join(".kivio").join("skills"),
        home.join(".claude").join("skills"),
        home.join(".cursor").join("skills"),
        home.join(".codex").join("skills"),
        home.join(".hermes").join("skills"),
        home.join(".openclaw").join("skills"),
        home.join(".opencode").join("skills"),
    ];
    for root in skill_roots {
        remove_named_dir(root.join("cua-driver"), "Skill", &mut out);
    }

    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        remove_named_dir(
            PathBuf::from(local).join("Programs").join("Cua"),
            "安装目录",
            &mut out,
        );
    }

    #[cfg(target_os = "macos")]
    {
        remove_named_dir(
            PathBuf::from("/Applications/CuaDriver.app"),
            "应用",
            &mut out,
        );
    }

    out
}

fn kill_named_processes(name: &str) {
    #[cfg(windows)]
    {
        let exe = if name.ends_with(".exe") {
            name.to_string()
        } else {
            format!("{name}.exe")
        };
        let _ = Command::new("taskkill")
            .args(["/IM", &exe, "/F", "/T"])
            .no_console_window()
            .output();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("pkill")
            .args(["-f", name])
            .no_console_window()
            .output();
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = name;
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// 官方 `officecli skills install` 把领域 skill 写到 `~/.agents/skills/<folder>`，
/// folder 是 `pptx` 而 catalog 里的 skill_id 是 `officecli-pptx`。
fn officecli_skill_folder(skill_id: &str) -> &str {
    OFFICECLI_DOMAIN_SKILLS
        .iter()
        .find(|(_, id)| *id == skill_id)
        .map(|(folder, _)| *folder)
        .unwrap_or(skill_id)
}

fn skill_folder_names(skill_id: &str) -> Vec<&str> {
    let alias = officecli_skill_folder(skill_id);
    if alias == skill_id {
        vec![skill_id]
    } else {
        vec![skill_id, alias]
    }
}

fn home_agent_skill_parents() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    vec![
        home.join(".agents").join("skills"),
        home.join(".kivio").join("skills"),
        home.join(".claude").join("skills"),
        home.join(".cursor").join("skills"),
        home.join(".codex").join("skills"),
        home.join(".copilot").join("skills"),
        home.join(".hermes").join("skills"),
        home.join(".openclaw").join("skills"),
        home.join(".opencode").join("skills"),
        home.join(".cua-driver").join("skills"),
    ]
}

/// 官方安装器 / `skills install` 写入的 skill 目录（含 `SKILL.md`）。
pub(crate) fn official_skill_dir(skill_id: &str) -> Option<PathBuf> {
    for folder in skill_folder_names(skill_id) {
        for parent in home_agent_skill_parents() {
            let dir = parent.join(folder);
            if dir.join("SKILL.md").is_file() {
                return Some(dir);
            }
        }
    }
    None
}

pub(crate) fn plugin_skill_present(plugin_id: &str, skill_id: &str) -> bool {
    skill_dir(plugin_id, skill_id)
        .map(|dir| dir.join("SKILL.md").is_file())
        .unwrap_or(false)
        || official_skill_dir(skill_id).is_some()
}

fn official_skill_install_argvs(plugin: &CatalogPlugin) -> Vec<Vec<&'static str>> {
    match plugin.id {
        "cua-driver" => vec![vec!["skills", "install", "--all-platforms"]],
        "officecli" => vec![vec!["skills", "install"]],
        _ => Vec::new(),
    }
}

/// 磁盘上已有任意一个官方 Skill 就不再跑 CLI。OfficeCLI catalog 有 12 个 id，
/// 缺 morph-ppt-3d 不应每次启用都重装整包。
fn official_skills_need_install(catalog: &CatalogPlugin) -> bool {
    catalog.uses_shared_skill_dirs()
        && !catalog.skill_ids.is_empty()
        && !official_skill_install_argvs(catalog).is_empty()
        && !plugin_has_any_official_or_copied_skill(catalog)
}

fn claim_official_skill_sync(id: &str) -> bool {
    OFFICIAL_SKILL_SYNC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string())
}

fn release_official_skill_sync(id: &str) {
    OFFICIAL_SKILL_SYNC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}

fn spawn_missing_official_skills(list: &[PluginStatus]) {
    for status in list {
        if status.enabled && status.has_skill && !status.skill_active {
            spawn_official_skill_sync(&status.id);
        }
    }
}

fn spawn_official_skill_sync(plugin_id: &str) {
    let Some(catalog) = catalog_plugin(plugin_id) else {
        return;
    };
    if !official_skills_need_install(catalog) {
        return;
    }
    let Some(bin) = resolve_binary(plugin_id).or_else(|| resolve_binary_for_status(plugin_id)) else {
        return;
    };
    if !claim_official_skill_sync(catalog.id) {
        return;
    }
    let plugin_id = catalog.id;
    tauri::async_runtime::spawn(async move {
        let result = ensure_official_skills(catalog, &bin).await;
        release_official_skill_sync(plugin_id);
        if let Err(err) = result {
            eprintln!("[plugins] official skills {plugin_id}: {err}");
        }
    });
}

/// 官方一键安装器只装二进制；启用 / 列表 / 安装后后台补跑 `skills install`。
async fn ensure_official_skills(catalog: &CatalogPlugin, binary: &Path) -> Result<(), String> {
    if !official_skills_need_install(catalog) {
        return Ok(());
    }
    for args in official_skill_install_argvs(catalog) {
        let mut cmd = tokio::process::Command::new(binary);
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .no_console_window();
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                eprintln!("[plugins] skills install spawn {}: {err}", catalog.id);
                break;
            }
        };
        match tokio::time::timeout(SKILLS_INSTALL_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    let detail = trim_install_output(&output.stdout, &output.stderr);
                    eprintln!(
                        "[plugins] skills install {} {:?} exit {:?}: {detail}",
                        catalog.id,
                        args,
                        output.status.code()
                    );
                }
            }
            Ok(Err(err)) => {
                eprintln!("[plugins] skills install {}: {err}", catalog.id);
            }
            Err(_) => {
                eprintln!("[plugins] skills install {} timed out", catalog.id);
            }
        }
        if plugin_has_any_official_or_copied_skill(catalog) {
            break;
        }
    }
    if plugin_has_any_official_or_copied_skill(catalog) {
        Ok(())
    } else {
        Err("官方 Skill 尚未就绪".to_string())
    }
}

fn spawn_plugin_mcp_warmup(app: &AppHandle, server_id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let server = {
            let settings = state.settings_read();
            settings
                .chat_tools
                .servers
                .iter()
                .find(|s| s.id == server_id)
                .cloned()
        };
        let Some(server) = server else {
            return;
        };
        if let Err(err) = state.mcp_list_tools(Some(&app), &server).await {
            eprintln!("[plugins] MCP warmup for {} failed: {err}", server.id);
        }
    });
}

fn plugin_has_any_official_or_copied_skill(catalog: &CatalogPlugin) -> bool {
    catalog
        .skill_ids
        .iter()
        .any(|id| plugin_skill_present(catalog.id, id))
}

/// 将插件附属 Skill 落到 `plugins/<id>/skills/`。
/// 官方共享目录型（OfficeCLI / Cua Driver）不拷贝：discover 已扫 `~/.agents/skills`。
pub(crate) fn write_skill_files(catalog: &CatalogPlugin) -> Result<(), String> {
    if catalog.skill_ids.is_empty() || catalog.uses_shared_skill_dirs() {
        return Ok(());
    }
    if catalog.skill_md.trim().is_empty() {
        return Ok(());
    }
    for skill_id in catalog.skill_ids {
        let dir = skill_dir(catalog.id, skill_id)
            .ok_or_else(|| "app data directory unavailable".to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("create skill dir: {e}"))?;
        std::fs::write(dir.join("SKILL.md"), catalog.skill_md)
            .map_err(|e| format!("write skill: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod skill_sync_tests {
    use super::{
        build_status, catalog_plugin, get_install_brief, officecli_skill_folder,
        official_install_program_args, official_skill_install_argvs, official_skills_need_install,
    };

    #[test]
    fn official_install_wraps_readme_command() {
        let script = "irm https://example.invalid/install.ps1 | iex";
        let (program, args) = official_install_program_args(script);
        #[cfg(windows)]
        {
            assert_eq!(program, "powershell.exe");
            assert!(args.windows(2).any(|pair| pair == ["-Command", script]));
        }
        #[cfg(not(windows))]
        {
            assert_eq!(program, "bash");
            assert_eq!(args, vec!["-lc".to_string(), script.to_string()]);
        }
    }

    #[test]
    fn officecli_folder_alias_maps_domain_ids() {
        assert_eq!(officecli_skill_folder("officecli-pptx"), "pptx");
        assert_eq!(officecli_skill_folder("officecli-docx"), "word");
        assert_eq!(officecli_skill_folder("officecli"), "officecli");
        assert_eq!(officecli_skill_folder("cua-driver"), "cua-driver");
    }

    #[test]
    fn cua_driver_brief_is_not_officecli_domain_skills() {
        let brief = get_install_brief("cua-driver").expect("brief");
        assert!(!brief.user_message.contains("morph-ppt"));
        assert!(!brief.user_message.contains("pitch-deck"));
        assert!(brief.user_message.contains("cua-driver skills install"));
        assert!(brief.user_message.contains("plugin-cua-driver"));
    }

    #[test]
    fn officecli_brief_still_lists_domain_skills() {
        let brief = get_install_brief("officecli").expect("brief");
        assert!(brief.user_message.contains("pitch-deck"));
        assert!(brief.user_message.contains("officecli skills install"));
    }

    #[test]
    fn cua_driver_skill_install_uses_all_platforms() {
        let p = catalog_plugin("cua-driver").expect("cua-driver");
        assert_eq!(
            official_skill_install_argvs(p),
            vec![vec!["skills", "install", "--all-platforms"]]
        );
        let office = catalog_plugin("officecli").expect("officecli");
        assert_eq!(
            official_skill_install_argvs(office),
            vec![vec!["skills", "install"]]
        );
        let ego = catalog_plugin("ego-lite").expect("ego-lite");
        assert!(official_skill_install_argvs(ego).is_empty());
        assert!(!official_skills_need_install(ego));
        assert!(office.skill_ids.len() > 1);
    }

    #[test]
    fn plugin_status_does_not_expose_install_command() {
        let catalog = catalog_plugin("cua-driver").expect("cua-driver");
        let status = build_status(catalog, &None, None, false, None);
        let value = serde_json::to_value(&status).expect("serialize");
        assert!(value.get("installCommand").is_none());
        assert_eq!(
            value["canInstall"],
            serde_json::Value::Bool(catalog.host_install_command().is_some())
        );
    }
}
