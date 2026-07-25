//! 插件状态列表、启用/卸载、以及 **给 AI 的安装 brief**。
//!
//! 安装本身不由后端静默下载：前端点「让 AI 安装」→ 开对话 → Agent 按 install_doc 执行。

use serde::Serialize;
use tauri::AppHandle;

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
use std::path::PathBuf;
use std::process::Command;

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
    // 有 skill_ids 即声明附属 skill（正文可来自 skill_md 或官方 CLI 同步）
    let has_skill = skill_count > 0;
    let has_mcp = mcp_count > 0;
    let skill_active = enabled
        && has_skill
        && catalog.skill_ids.iter().any(|sid| {
            super::state::skill_dir(catalog.id, sid)
                .map(|d| d.join("SKILL.md").is_file())
                .unwrap_or(false)
        });
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
    Ok(list)
}

/// 缓存态 + mcp_active（无 spawn）。前端首屏用。
pub fn list_plugin_statuses_cached_with_state(
    state: &AppState,
) -> Result<Vec<PluginStatus>, String> {
    let mut list = list_plugin_statuses_cached()?;
    fill_mcp_active(&mut list, state);
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
| **A. 安装（本对话）** | **你（AI）** | ① 按 README 安装官方 `{binary}` ② 用官方 CLI **安装官方 Skills** ③ 验收并汇报 |
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

### 3.2 Skill：必须由你用官方 CLI **全量**装好（不要跳过、不要只装常用三个）

官方技能包由 CLI 管理。安装二进制之后，**必须把列表里每一个 skill 都装上**（用户期望装插件 = 功能齐套）：

1. **base**：
   ```
   {binary} skills install
   ```
2. **全部领域 skill**（逐个 `skills install <name>`，缺一不可）：
   `pptx` `word` `excel` `morph-ppt` `morph-ppt-3d` `pitch-deck` `academic-paper` `data-dashboard` `financial-model` `word-form`

   PowerShell 示例：
   ```
   foreach ($s in @('pptx','word','excel','morph-ppt','morph-ppt-3d','pitch-deck','academic-paper','data-dashboard','financial-model','word-form')) {{ {binary} skills install $s }}
   ```
3. 验收：
   ```
   {binary} skills list
   {binary} load_skill pptx
   {binary} load_skill pitch-deck
   ```
   - `skills list` 中上列 skill **全部**为 installed（允许额外 skill 也装）。
   - `load_skill` 抽查应打出完整 SKILL 正文。

说明：

- **禁止**只装 pptx/word/excel 就收工。
- **禁止**自己编写「精简 SKILL.md」代替官方内容。
- **禁止**用 python-docx 等替代本插件。
- 用户启用后，Kivio 接入全部官方 skill，供对话里 `skill` 激活。

## 4. 安装步骤清单（按序执行）

### 4.1 二进制

1. 按 README 官方方式安装（一键脚本 / brew / scoop / Release 等，以 README 为准）。
2. 验收并贴出输出：
   ```
   {binary} --version
   ```
   尽量给出可执行文件完整路径。
3. 若已安装且 version 正常：报告版本与路径，**不要重复安装**（除非用户要求升级）。

### 4.2 官方 Skills（本对话内由你执行）

见 §3.2。装完 base + **全部**领域 skill，并用 `skills list` 确认全为 installed。

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
        return Err(
            "尚未检测到插件命令。请先点「让 AI 安装」，装完后点刷新；若已装在默认目录，刷新应能识别。".to_string(),
        );
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

    let mut skill_sync_err: Option<String> = None;
    if enabled {
        // 下载型插件（如 ego lite）：从仓库拉 Skill 到插件目录；失败不阻断启用，提示用户。
        if let Some(url) = catalog.skill_download_url {
            if let Err(err) = download_plugin_skill(catalog, url).await {
                skill_sync_err = Some(err);
                eprintln!(
                    "[plugins] skill download on enable {}: {}",
                    id,
                    skill_sync_err.as_deref().unwrap_or("")
                );
            }
        }
        // Skill 同步失败不阻断 MCP 启用；提示用户走「让 AI 安装」补 Skills
        if let Err(err) = write_skill_files(catalog) {
            skill_sync_err = Some(err);
            eprintln!(
                "[plugins] skill sync on enable {}: {}",
                id,
                skill_sync_err.as_deref().unwrap_or("")
            );
        }
        apply_enable_side_effects(app, state, id)?;
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
    status.skill_active = enabled
        && status.has_skill
        && catalog.skill_ids.iter().any(|sid| {
            super::state::skill_dir(catalog.id, sid)
                .map(|d| d.join("SKILL.md").is_file())
                .unwrap_or(false)
        });

    let message = if enabled {
        let mut parts = vec![format!("已启用 {}", catalog.name)];
        if status.skill_active {
            parts.push(format!(
                "官方 Skill 已接入：{}",
                catalog.skill_ids.join(", ")
            ));
        } else if status.has_skill {
            let detail = skill_sync_err
                .as_deref()
                .map(|e| format!("（{e}）"))
                .unwrap_or_default();
            parts.push(format!(
                "Skill 尚未接入{detail}：请点「让 AI 安装」，让 AI 执行官方 `skills install`（base + 全部领域 skill）后重新启用"
            ));
        }
        if status.has_mcp {
            if status.mcp_active {
                parts.push(format!(
                    "官方 MCP 已注册（`{} mcp`，id={}）——无需 mcp claude/cursor",
                    catalog.binary,
                    status.mcp_server_id.as_deref().unwrap_or("?")
                ));
            } else {
                parts.push("MCP 注册失败，请重试启用".to_string());
            }
        }
        parts.push("系统提示已挂载；请新开对话使用".to_string());
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
        // 给进程退出一点时间
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

    // 3) OfficeCLI 残留：配置与写入各 Agent 的官方 skills
    if id == "officecli" {
        cleaned.extend(remove_officecli_residuals());
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

/// 将插件附属 Skill 落到 `plugins/<id>/skills/`。
/// - 若 `skill_md` 非空：写入每个 skill_id（旧路径 / 简单插件）
/// - OfficeCLI：`skill_md` 为空 → 从**官方二进制**同步 base + 领域 skill（禁止 Kivio 手写 stub）
pub(crate) fn write_skill_files(catalog: &CatalogPlugin) -> Result<(), String> {
    if catalog.skill_ids.is_empty() {
        return Ok(());
    }
    if catalog.id == "officecli" && catalog.skill_md.trim().is_empty() {
        return sync_officecli_official_skills();
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

/// 从已安装的 `officecli` 拉取官方 Skill，写入 Kivio 插件 skill 目录。
///
/// - base `officecli`：官方 `skills install` 落到各 Agent 目录的 SKILL.md（复制进来）
/// - 领域 `pptx` / `word` / `excel`：`officecli load_skill <name>` 导出全文
fn sync_officecli_official_skills() -> Result<(), String> {
    let binary = resolve_binary_for_status("officecli")
        .or_else(|| resolve_binary("officecli"))
        .ok_or_else(|| "officecli binary not found; cannot sync official skills".to_string())?;

    // 不在这里静默执行 `skills install`：安装阶段由「Kivio AI 安装」对话按 install_brief 执行。
    // 启用时只把 AI/官方已装好的 skill 接入 Kivio 目录。
    let base_md = find_official_officecli_base_skill()
        .ok_or_else(|| {
            "official officecli base skill not found. Use 扩展 → 插件 → 让 AI 安装, and ensure the agent ran `officecli skills install` (base), then re-enable.".to_string()
        })?;
    write_plugin_skill_md("officecli", "officecli", &base_md)?;

    // 全量领域 skill：CLI 名 → frontmatter id（见 catalog::OFFICECLI_DOMAIN_SKILLS）
    let mut errors: Vec<String> = Vec::new();
    for (cli_name, skill_id) in OFFICECLI_DOMAIN_SKILLS {
        let output = Command::new(&binary)
            .args(["load_skill", cli_name])
            .no_console_window()
            .output()
            .map_err(|e| format!("officecli load_skill {cli_name}: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            errors.push(format!("{cli_name}: {}", err.trim()));
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        match extract_skill_markdown(&text) {
            Some(md) => {
                if let Err(e) = write_plugin_skill_md("officecli", skill_id, md) {
                    errors.push(format!("{cli_name}: {e}"));
                }
            }
            None => errors.push(format!("{cli_name}: output missing SKILL.md frontmatter")),
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "some official skills failed to sync: {}",
            errors.join("; ")
        ));
    }
    Ok(())
}

fn write_plugin_skill_md(plugin_id: &str, skill_id: &str, body: &str) -> Result<(), String> {
    let dir = skill_dir(plugin_id, skill_id)
        .ok_or_else(|| "app data directory unavailable".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create skill dir: {e}"))?;
    std::fs::write(dir.join("SKILL.md"), body)
        .map_err(|e| format!("write skill {skill_id}: {e}"))?;
    Ok(())
}

/// 官方 base skill 常见落点（`officecli skills install` / 安装器写入）。
fn find_official_officecli_base_skill() -> Option<String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    let candidates = [
        home.join(".agents")
            .join("skills")
            .join("officecli")
            .join("SKILL.md"),
        home.join(".claude")
            .join("skills")
            .join("officecli")
            .join("SKILL.md"),
        home.join(".cursor")
            .join("skills")
            .join("officecli")
            .join("SKILL.md"),
        home.join(".copilot")
            .join("skills")
            .join("officecli")
            .join("SKILL.md"),
        home.join(".codex")
            .join("skills")
            .join("officecli")
            .join("SKILL.md"),
        home.join(".hermes")
            .join("skills")
            .join("officecli")
            .join("SKILL.md"),
        home.join(".openclaw")
            .join("skills")
            .join("officecli")
            .join("SKILL.md"),
    ];
    for path in candidates {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if raw.contains("name: officecli") || raw.contains("name:officecli") {
                return Some(raw);
            }
        }
    }
    None
}

/// 从 `load_skill` stdout 取出以 YAML frontmatter 开头的 SKILL.md 正文。
fn extract_skill_markdown(stdout: &str) -> Option<&str> {
    let trimmed = stdout.trim();
    if trimmed.starts_with("---") {
        return Some(trimmed);
    }
    // 若前面有 banner，取第一个 frontmatter 起
    if let Some(idx) = trimmed.find("\n---\n") {
        return Some(trimmed[idx + 1..].trim());
    }
    if let Some(idx) = trimmed.find("\r\n---\r\n") {
        return Some(trimmed[idx + 2..].trim());
    }
    None
}

#[cfg(test)]
mod skill_sync_tests {
    use super::extract_skill_markdown;

    #[test]
    fn extract_skill_from_plain_stdout() {
        let s = "---\nname: officecli-pptx\n---\n\n# Hi\n";
        assert!(extract_skill_markdown(s).unwrap().starts_with("---"));
    }

    #[test]
    fn extract_skill_skips_banner() {
        let s = "Loading...\n---\nname: officecli-pptx\n---\n\n# Hi\n";
        let md = extract_skill_markdown(s).unwrap();
        assert!(md.starts_with("---"));
        assert!(md.contains("officecli-pptx"));
    }
}
