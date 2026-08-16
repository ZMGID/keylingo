//! Kivio 专用 dsh profile 的生成与维护。
//!
//! dsh 没有「一条命令直接出流式 JSON」的模式，它**只能 boot profile**（`dsh --profile <name>`）。
//! 一个 profile 是 `$DSH_HOME/profiles/<name>/` 下的目录：`package.json`（依赖 + `dsh.profile.bundles`
//! 层列表）+ 用户自己的 `cordis.patch.yml`。
//!
//! # 三条边界
//!
//! 1. **复用用户的 `$DSH_HOME`**（默认 `~/.dsh`），因为 `settings.yaml` 里的供应商配置与
//!    `.credentials.yaml` / `.env` 里的 key 都在那儿 —— 换一个私有 home 就等于要求用户再配一遍。
//! 2. **只写 `profiles/kivio/`**。用户自己的 `profiles/web`、`profiles/tui` 与家目录那份
//!    `cordis.patch.yml` 一律不碰（同 Kivio 从不改写 `~/.claude` / `~/.codex` 的既有红线）。
//! 3. **依赖安装走官方 `dsh plugin --profile` 通道**，不自己写 `package.json` + `pnpm install`。
//!    那条命令做三件我们自己做会漏的事：按模板初始化 profile（`dsh.profile.bundles` 要写
//!    `@deepseek-ai/dsh-base`，**少了它 profile 起来没有 agent**）、转发 pnpm、按安装结果回填
//!    bundle 层列表。本机实测漏掉 bundles 那次的表现是进程能起、`initialize` 能回，
//!    但一 prompt 就没有任何 agent 接。安装前会写 `profiles/kivio/.npmrc` 钉死
//!    `registry.npmjs.org`，避免用户家目录镜像缺 `@deepseek-ai/*`。
//!
//! # patch 里放什么
//!
//! 包含 Kivio profile 的装配与进程级配置：
//! - `kivio-dsh-bridge.mjs` 一条 insert（在官方 server 上补 `resume` / `cancel` / `command` /
//!   `commands` / `stop-job` RPC）
//! - `hmr` 关掉：那是给开发热重载用的，常驻会话里只会带来意外重连
//! - `session-title-llm` 关掉：Kivio 自己起标题，留着等于每轮多付一次模型调用
//!   （实测确认它会发 `session/title-llm-request`）
//! - `llm-deepseek.reasoningEffort`：官方 DeepSeek 路由的档位（只认 `off|high|max`）。
//!   第三方 `llm-pi-ai` 路由另写 `providers.<route>.reasoning`（pi-ai 档位表）。
//!   都不是启动 flag，也不在 `initialize` 参数里，所以换档位必须重写这个文件并换进程。
//! - `agent-presets`：四档 Agent 模式（standard / code / minimal / cordis）。与官方 web
//!   一样关掉 host 平面工具，改由所选 preset 组装；`dsh --profile` 会把随包的
//!   `config/agent-presets` 补进 `roots`。
//! - 当前 Kivio 供应商的 `llm-pi-ai.providers.<route>`：只挂在 `profiles/kivio`，Key 仅以
//!   `apiKeyEnv` 引用并由 Kivio 注入进程，不写入 YAML。
//!
//! **当前选择的模型不写进 patch**。provider 的模型目录在 patch 中，但每轮实际 route/model
//! 仍由 `initialize` RPC 决定（见 `session::dsh_jsonrpc`）。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::proc::NoConsoleWindow;
use crate::settings::ExternalCliProvider;

/// Kivio 私有 profile 名。与用户的 `web` / `tui` 并存。
pub const KIVIO_PROFILE: &str = "kivio";

const BRIDGE_FILENAME: &str = "kivio-dsh-bridge.mjs";
const BRIDGE_SOURCE: &str = include_str!("../../resources/dsh/kivio-dsh-bridge.mjs");

/// Bridge 复用官方 server/transport；dsh core API 由运行中的 profile 提供，避免安装第二份 core。
const REQUIRED_PACKAGES: &[&str] = &[
    "@deepseek-ai/dsh-sdk-jsonrpc-server",
    "@deepseek-ai/dsh-sdk-protocol",
];

/// 不要把 `@deepseek-ai/dsh-agent-presets` 装进 kivio profile。
/// `dsh plugin add` 不带版本会落到 `0.0.1-rc.1`，和本机 dsh 带的 `0.1.0-rc.6` 抢解析，
/// 然后缺 `@deepseek-ai/dsh-paths`。patch 里只写插件名，让官方 module fallback 用随包那份。
const PROFILE_PRESET_PACKAGE: &str = "@deepseek-ai/dsh-agent-presets";

/// pnpm 在 profile 目录里读这份，盖过用户家目录 `.npmrc` 的淘宝镜像。
const PROFILE_NPMRC: &str = "registry=https://registry.npmjs.org/\n\
@deepseek-ai:registry=https://registry.npmjs.org/\n";

/// dsh web 把这些 host 平面工具关掉，改由 agent preset 按会话组装。
const HOST_PLANE_TOOL_IDS: &[&str] = &[
    "tool-bash",
    "tool-pwsh",
    "tool-jobs",
    "tool-fs",
    "tool-fs-search",
    "tool-str-replace-editor",
    "skill-filesystem",
    "tool-skill",
    "tool-goal",
    "plan-mode",
    "compaction-basic",
    "command-compact",
    "tool-result-pruner",
    "tool-subagent-control",
    "tool-subagent-list-agents",
    "tool-subagent",
    "tool-subagent-fork",
    "workflow-worker-thread",
    "tool-workflow",
    "tool-ralph",
    "agent-instructions",
    "tool-todo",
    "tool-web",
];

/// `$DSH_HOME`，未设时 `~/.dsh`（与上游 `resolveDshHome` 同序）。
fn dsh_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("DSH_HOME") {
        let path = PathBuf::from(home);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".dsh"))
}

/// `$DSH_HOME/profiles/kivio/`。
pub fn profile_dir() -> Option<PathBuf> {
    dsh_home().map(|home| home.join("profiles").join(KIVIO_PROFILE))
}

/// 合法 preset id；其它值（含空）回落 `standard`。
pub fn normalize_agent_preset(value: Option<&str>) -> &'static str {
    match value.map(str::trim) {
        Some("code") => "code",
        Some("minimal") => "minimal",
        Some("cordis") => "cordis",
        _ => "standard",
    }
}

/// profile 的 `cordis.patch.yml` 内容。
///
/// `reasoning` 为 `None` / 空 / `"default"` 时**不写** `llm-deepseek` 那条 —— 让适配器用它
/// 自己的默认（实测 `defaultEffort: high`）。写一个 `default` 字面量会被 schema 拒（合法值
/// 只有 `off|high|max`），整棵树起不来。
fn render_patch(reasoning: Option<&str>, preset: Option<&str>) -> String {
    let mut out = String::from(
        "# 由 Kivio 生成并维护，请勿手改（每次启动 dsh 会话时按当前设置重写）。\n\
         #\n\
         # 用户自己的 profile（web / tui）与 $DSH_HOME/cordis.patch.yml 不受影响。\n\
         \n\
         # 开发用热重载：常驻会话里只会带来意外重连。\n\
         - id: hmr\n\
         \x20 disabled: true\n\
         \n\
         # 标题由 Kivio 自己起；留着等于每轮多付一次模型调用。\n\
         - id: session-title-llm\n\
         \x20 disabled: true\n",
    );
    if let Some(effort) = normalize_reasoning_effort(reasoning) {
        out.push_str(
            "\n# 推理档位唯一入口（不是启动 flag，也不在 initialize 参数里）。\n\
             - id: llm-deepseek\n\
             \x20 config:\n\
             \x20   reasoningEffort: ",
        );
        out.push_str(effort);
        out.push('\n');
    }
    out.push_str("\n# 与官方 web 相同：host 平面工具关掉，改由下面的 agent preset 按会话组装。\n");
    for id in HOST_PLANE_TOOL_IDS {
        out.push_str("- id: ");
        out.push_str(id);
        out.push_str("\n  disabled: true\n");
    }
    out.push_str(
        "\n# 四档 Agent 模式。随包 roots 由 `dsh --profile` 的 composeProfile 注入。\n\
         - insert:\n\
         \x20   - id: agent-presets\n\
         \x20     name: '@deepseek-ai/dsh-agent-presets'\n\
         \x20     config:\n\
         \x20       default: ",
    );
    out.push_str(normalize_agent_preset(preset));
    out.push_str(
        "\n\n# Kivio bridge：保留官方事件流，并补齐跨进程 resume 与协议级 cancel。\n\
         - insert:\n\
         \x20   - id: kivio-dsh-jsonrpc-bridge\n\
         \x20     name: './kivio-dsh-bridge.mjs'\n",
    );
    out
}

#[derive(Debug)]
struct ActiveDshProvider {
    route: String,
    default_model: String,
    config: Value,
}

fn parse_active_provider(provider: &ExternalCliProvider) -> Result<ActiveDshProvider, String> {
    let route = provider.native_provider_id.trim();
    if route.is_empty()
        || route.len() > 64
        || !route.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        || !route
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !route
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(
            "dsh 供应商 ID 只能使用小写字母、数字、点、下划线和连字符，且不能以符号开头或结尾"
                .to_string(),
        );
    }
    let config: Value = serde_json::from_str(&provider.config_json)
        .map_err(|err| format!("dsh 供应商 {} 的 configJson 无效：{err}", provider.name))?;
    let object = config
        .as_object()
        .ok_or_else(|| format!("dsh 供应商 {} 的 configJson 必须是对象", provider.name))?;
    if object.contains_key("apiKey") || object.contains_key("api_key") {
        return Err(format!(
            "dsh 供应商 {} 不能把 API Key 写进 configJson；请使用 apiKeyEnv",
            provider.name
        ));
    }
    let api_key_env = object
        .get("apiKeyEnv")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("dsh 供应商 {} 缺少 apiKeyEnv", provider.name))?;
    if !provider
        .env
        .iter()
        .any(|pair| pair.key == api_key_env && !pair.value.trim().is_empty())
    {
        return Err(format!(
            "dsh 供应商 {} 缺少环境变量 {api_key_env} 的 API Key",
            provider.name
        ));
    }
    for field in ["api", "baseURL"] {
        if !object
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(format!("dsh 供应商 {} 缺少 {field}", provider.name));
        }
    }
    let models = object
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty())
        .ok_or_else(|| format!("dsh 供应商 {} 至少需要一个模型", provider.name))?;
    let default_model = provider.default_model.trim();
    if default_model.is_empty()
        || !models.iter().any(|model| {
            model.get("id").and_then(Value::as_str).map(str::trim) == Some(default_model)
        })
    {
        return Err(format!(
            "dsh 供应商 {} 的默认模型不在模型列表中",
            provider.name
        ));
    }
    Ok(ActiveDshProvider {
        route: route.to_string(),
        default_model: default_model.to_string(),
        config,
    })
}

fn render_patch_with_provider(
    reasoning: Option<&str>,
    preset: Option<&str>,
    provider: Option<&ExternalCliProvider>,
) -> Result<String, String> {
    let mut out = render_patch(reasoning, preset);
    let Some(provider) = provider else {
        return Ok(out);
    };
    let active = parse_active_provider(provider)?;
    let mut config = active.config;
    if let Some(object) = config.as_object_mut() {
        if let Some(effort) = normalize_pi_reasoning_effort(reasoning) {
            object.insert(
                "reasoning".to_string(),
                Value::String(effort.to_string()),
            );
        } else {
            object.remove("reasoning");
        }
    }
    let yaml = serde_yaml::to_string(&config)
        .map_err(|err| format!("序列化 dsh 供应商配置失败：{err}"))?;
    out.push_str("\n# 仅挂载到 Kivio profile 的第三方供应商；API Key 只通过环境变量注入。\n");
    out.push_str("- id: llm-pi-ai\n  config:\n    providers:\n      ");
    out.push_str(&active.route);
    out.push_str(":\n");
    for line in yaml.trim_start_matches("---\n").lines() {
        out.push_str("        ");
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

pub fn active_provider_default_route() -> Result<Option<(String, String)>, String> {
    let Some(provider) = crate::external_agents::overrides::active_provider("dsh") else {
        return Ok(None);
    };
    let active = parse_active_provider(&provider)?;
    Ok(Some((active.route, active.default_model)))
}

/// 空 / `"default"` / 未知值视为「不指定」，与 `types::normalize_model` 同语义。
///
/// 只认 `defs/dsh.rs` 的合法档位 `off|high|max`。其它字符串（含换行）绝不能写进
/// 共享的 `cordis.patch.yml`：那是顶层 YAML 数组，插一行就能重开 host-plane 工具。
fn normalize_reasoning_effort(value: Option<&str>) -> Option<&'static str> {
    match value.map(str::trim) {
        Some("off") => Some("off"),
        Some("high") => Some("high"),
        Some("max") => Some("max"),
        _ => None,
    }
}

/// `llm-pi-ai` 供应商级 `reasoning`：官方档位表 `off|minimal|low|medium|high|xhigh|max`。
/// 未知值整段省略，避免把任意字符串写进共享 patch。
fn normalize_pi_reasoning_effort(value: Option<&str>) -> Option<&'static str> {
    match value.map(str::trim) {
        Some("off") => Some("off"),
        Some("minimal") => Some("minimal"),
        Some("low") => Some("low"),
        Some("medium") => Some("medium"),
        Some("high") => Some("high"),
        Some("xhigh") => Some("xhigh"),
        Some("max") => Some("max"),
        _ => None,
    }
}

/// 该 profile 的依赖是否已装好（两个包都要在 `node_modules` 里）。
fn packages_installed(dir: &Path) -> bool {
    REQUIRED_PACKAGES.iter().all(|pkg| {
        let relative = pkg.trim_start_matches('@');
        let (scope, name) = match relative.split_once('/') {
            Some(parts) => parts,
            None => return false,
        };
        dir.join("node_modules")
            .join(format!("@{scope}"))
            .join(name)
            .exists()
    })
}

/// 确保 profile 可用：装依赖（首次）+ 按当前设置重写 patch。
///
/// **幂等**。依赖已在时不跑 pnpm（那条要几秒）；patch 每次都重写，因为推理档位可能变了。
///
/// `Err` 的内容是给用户看的：pnpm 没装、网络不通、目录没权限都会走到这里，
/// 而这条错误会成为那一轮的失败气泡 —— 所以不写「bootstrap failed」这种查不到根因的话。
pub async fn ensure_profile_ready(
    bin: &Path,
    reasoning: Option<&str>,
    preset: Option<&str>,
) -> Result<(), String> {
    let dir =
        profile_dir().ok_or_else(|| "无法定位 dsh 主目录（$DSH_HOME / ~/.dsh）".to_string())?;

    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 dsh profile 目录失败：{e}"))?;
    std::fs::write(dir.join(".npmrc"), PROFILE_NPMRC)
        .map_err(|e| format!("写入 dsh profile npmrc 失败：{e}"))?;

    if !packages_installed(&dir) {
        install_packages(bin).await?;
    }
    strip_profile_agent_presets_dep(&dir);

    // patch 在依赖之后写：`dsh plugin` 首次会按模板初始化 profile 目录并写一份占位
    // `cordis.patch.yml`（内容是 `[]`），先写就会被它覆盖掉。
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 dsh profile 目录失败：{e}"))?;
    std::fs::write(dir.join(BRIDGE_FILENAME), BRIDGE_SOURCE)
        .map_err(|e| format!("写入 Kivio dsh bridge 失败：{e}"))?;
    let provider = crate::external_agents::overrides::active_provider("dsh");
    let patch = render_patch_with_provider(reasoning, preset, provider.as_ref())?;
    std::fs::write(dir.join("cordis.patch.yml"), patch)
        .map_err(|e| format!("写入 dsh profile 配置失败：{e}"))
}

fn strip_profile_agent_presets_dep(dir: &Path) {
    let package_json = dir.join("package.json");
    if let Ok(raw) = std::fs::read_to_string(&package_json) {
        if let Ok(mut value) = serde_json::from_str::<Value>(&raw) {
            let removed = value
                .get_mut("dependencies")
                .and_then(Value::as_object_mut)
                .is_some_and(|deps| deps.remove(PROFILE_PRESET_PACKAGE).is_some());
            if removed {
                if let Ok(next) = serde_json::to_string_pretty(&value) {
                    let _ = std::fs::write(&package_json, format!("{next}\n"));
                }
            }
        }
    }
    let relative = PROFILE_PRESET_PACKAGE.trim_start_matches('@');
    if let Some((scope, name)) = relative.split_once('/') {
        let _ = std::fs::remove_dir_all(
            dir.join("node_modules")
                .join(format!("@{scope}"))
                .join(name),
        );
    }
}

/// `dsh plugin --profile kivio add <pkgs>`。
///
/// 用探测到的那个绝对二进制（与跑轮次的是同一个），不走 PATH 再查一次。
fn remove_provider_env_from_install(
    command: &mut tokio::process::Command,
    provider: Option<&ExternalCliProvider>,
) {
    if let Some(provider) = provider {
        for env in &provider.env {
            command.env_remove(&env.key);
        }
    }
}

async fn install_packages(bin: &Path) -> Result<(), String> {
    use tokio::io::AsyncReadExt;

    crate::external_agents::installer::expose_node_on_path();
    crate::external_agents::installer::ensure_pnpm_for_dsh().await?;

    // Installation never needs model credentials. Build a clean command so pnpm and dependency
    // lifecycle scripts cannot inherit the active Kivio provider API key.
    let mut command = tokio::process::Command::new(bin);
    crate::external_agents::spawn::strip_parent_session_env(&mut command);
    crate::external_agents::installer::pin_official_npm_registry(&mut command);
    let provider = crate::external_agents::overrides::active_provider("dsh");
    remove_provider_env_from_install(&mut command, provider.as_ref());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    command
        .arg("plugin")
        .arg("--profile")
        .arg(KIVIO_PROFILE)
        .arg("add")
        .args(REQUIRED_PACKAGES)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_console_window();

    let mut child = command
        .spawn()
        .map_err(|e| format!("无法执行 dsh plugin：{e}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = stdout {
            let _ = pipe.read_to_end(&mut bytes).await;
        }
        bytes
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = stderr {
            let _ = pipe.read_to_end(&mut bytes).await;
        }
        bytes
    });

    let status = match tokio::time::timeout(std::time::Duration::from_secs(180), child.wait()).await
    {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => {
            crate::external_agents::spawn::kill_agent_process_tree(&mut child);
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(format!("等待 dsh plugin 失败：{err}"));
        }
        Err(_) => {
            crate::external_agents::spawn::kill_agent_process_tree(&mut child);
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err("安装 dsh 插件超时（180s）".to_string());
        }
    };
    let _stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();

    if status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&stderr);
    let tail = stderr.trim();
    if pnpm_missing_in_output(status.code(), tail) {
        return Err(
            "dsh 需要 pnpm 来安装 profile 插件，但 PATH 上没有找到 pnpm。\
             请到设置 → 本地 CLI Agent → DeepSeek Harness 点一次安装，或先运行 npm install -g pnpm 后重试。"
                .to_string(),
        );
    }
    Err(crate::external_agents::spawn::fold_stderr(
        "安装 dsh 插件失败".to_string(),
        tail,
    ))
}

fn pnpm_missing_in_output(status_code: Option<i32>, tail: &str) -> bool {
    status_code == Some(127)
        || tail.contains("pnpm not found")
        || tail.contains("pnpm failed")
        || tail.contains("'pnpm'")
        || tail.contains("\"pnpm\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay_provider() -> ExternalCliProvider {
        ExternalCliProvider {
            id: "p-relay".to_string(),
            name: "Relay".to_string(),
            native_provider_id: "relay-one".to_string(),
            default_model: "gpt-test".to_string(),
            config_json: serde_json::json!({
                "displayName": "Relay",
                "apiKeyEnv": "KIVIO_DSH_RELAY_ONE_API_KEY",
                "api": "openai-responses",
                "baseURL": "https://relay.example/v1",
                "models": [{
                    "id": "gpt-test",
                    "name": "GPT Test",
                    "contextWindow": 256000,
                    "maxTokens": 32768
                }]
            })
            .to_string(),
            env: vec![crate::settings::CliEnvVar {
                key: "KIVIO_DSH_RELAY_ONE_API_KEY".to_string(),
                value: "sk-secret".to_string(),
            }],
            ..Default::default()
        }
    }

    /// 装配 bridge 的 insert 必须在，否则 `initialize` 没有 resumable server 应答。
    #[test]
    fn patch_always_mounts_the_kivio_bridge() {
        let yml = render_patch(None, None);
        assert!(yml.contains("- insert:"));
        assert!(yml.contains("name: './kivio-dsh-bridge.mjs'"));
        assert!(yml.contains("id: kivio-dsh-jsonrpc-bridge"));
        assert!(yml.contains("name: '@deepseek-ai/dsh-agent-presets'"));
        assert!(yml.contains("default: standard"));
        assert!(yml.contains("- id: tool-bash\n  disabled: true"));
    }

    #[test]
    fn bridge_source_owns_resume_cancel_and_parent_cleanup() {
        assert!(BRIDGE_SOURCE.contains("ctx.agents.resume"));
        assert!(BRIDGE_SOURCE.contains("agent.cancel({ kind: 'user' })"));
        assert!(BRIDGE_SOURCE.contains("process.ppid !== parentPid"));
        assert!(BRIDGE_SOURCE.contains("input.once('end', onInputClosed)"));
        assert!(BRIDGE_SOURCE.contains("DSH_AGENT_PRESET"));
        assert!(BRIDGE_SOURCE.contains("agentPresets"));
        assert!(BRIDGE_SOURCE.contains("registerProvider"));
        assert!(BRIDGE_SOURCE.contains("session/ask"));
        assert!(BRIDGE_SOURCE.contains("'userQuestions'"));
        assert!(BRIDGE_SOURCE.contains("'attachments'"));
        assert!(BRIDGE_SOURCE.contains("'jobs'"));
        assert!(BRIDGE_SOURCE.contains("'subagents'"));
        assert!(BRIDGE_SOURCE.contains("saveImage"));
        assert!(BRIDGE_SOURCE.contains("session/commands"));
        assert!(BRIDGE_SOURCE.contains("session/stop-job"));
        assert!(BRIDGE_SOURCE.contains("commands.list"));
        assert!(BRIDGE_SOURCE.contains("jobs.kill"));
        assert!(BRIDGE_SOURCE.contains("subagents.interrupt"));
        assert!(!BRIDGE_SOURCE.contains("waitForProviderAdapter"));
        assert!(!BRIDGE_SOURCE.contains("@deepseek-ai/dsh-session"));
    }

    #[test]
    fn plugin_install_explicitly_removes_provider_key_env() {
        let provider = relay_provider();
        let mut command = tokio::process::Command::new("dsh");
        remove_provider_env_from_install(&mut command, Some(&provider));
        let key = std::ffi::OsStr::new("KIVIO_DSH_RELAY_ONE_API_KEY");
        let entry = command.as_std().get_envs().find(|(name, _)| *name == key);
        assert!(matches!(entry, Some((_, None))));
    }

    #[test]
    fn provider_patch_mounts_llm_pi_ai_without_serializing_the_api_key() {
        let provider = relay_provider();
        let yml = render_patch_with_provider(Some("high"), None, Some(&provider)).unwrap();
        assert!(yml.contains("- id: llm-pi-ai"));
        assert!(yml.contains("relay-one:"));
        assert!(yml.contains("apiKeyEnv: KIVIO_DSH_RELAY_ONE_API_KEY"));
        assert!(yml.contains("baseURL: https://relay.example/v1"));
        assert!(yml.contains("id: gpt-test"));
        assert!(yml.contains("reasoning: high"));
        assert!(!yml.contains("sk-secret"));
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yml).unwrap();
        assert!(parsed.is_sequence());
    }

    #[test]
    fn provider_patch_writes_pi_ai_reasoning_for_custom_efforts() {
        let provider = relay_provider();
        let yml = render_patch_with_provider(Some("low"), None, Some(&provider)).unwrap();
        assert!(yml.contains("reasoning: low"));
        assert!(!yml.contains("reasoningEffort:"));
        assert!(!yml.contains("- id: llm-deepseek"));

        let yml = render_patch_with_provider(Some("xhigh"), None, Some(&provider)).unwrap();
        assert!(yml.contains("reasoning: xhigh"));

        let yml = render_patch_with_provider(Some("default"), None, Some(&provider)).unwrap();
        assert!(!yml.contains("reasoning:"));
    }

    #[test]
    fn provider_patch_rejects_missing_key_and_invalid_default_model() {
        let mut provider = relay_provider();
        provider.env.clear();
        assert!(render_patch_with_provider(None, None, Some(&provider))
            .unwrap_err()
            .contains("KIVIO_DSH_RELAY_ONE_API_KEY"));

        let mut provider = relay_provider();
        provider.default_model = "missing".to_string();
        assert!(render_patch_with_provider(None, None, Some(&provider))
            .unwrap_err()
            .contains("默认模型"));
    }

    /// 两条 disable 是钱和稳定性：标题 LLM 每轮一次白花的调用，hmr 是意外重连。
    #[test]
    fn patch_disables_hmr_and_title_llm() {
        let yml = render_patch(None, None);
        assert!(yml.contains("- id: hmr\n  disabled: true"));
        assert!(yml.contains("- id: session-title-llm\n  disabled: true"));
    }

    #[test]
    fn strip_profile_agent_presets_dep_removes_the_shadowing_copy() {
        let dir =
            std::env::temp_dir().join(format!("kivio-dsh-preset-strip-{}", std::process::id()));
        let pkg_dir = dir
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-agent-presets");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("package.json"), "{}").unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"@deepseek-ai/dsh-agent-presets":"0.0.1-rc.1","@deepseek-ai/dsh-sdk-protocol":"0.0.1-rc.1"}}"#,
        )
        .unwrap();

        strip_profile_agent_presets_dep(&dir);

        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("package.json")).unwrap())
                .unwrap();
        assert!(value["dependencies"]
            .get("@deepseek-ai/dsh-agent-presets")
            .is_none());
        assert_eq!(
            value["dependencies"]["@deepseek-ai/dsh-sdk-protocol"],
            "0.0.1-rc.1"
        );
        assert!(!pkg_dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_agent_preset_accepts_only_the_four_ids() {
        assert_eq!(normalize_agent_preset(None), "standard");
        assert_eq!(normalize_agent_preset(Some("")), "standard");
        assert_eq!(normalize_agent_preset(Some("default")), "standard");
        assert_eq!(normalize_agent_preset(Some("code")), "code");
        assert_eq!(normalize_agent_preset(Some("minimal")), "minimal");
        assert_eq!(normalize_agent_preset(Some("cordis")), "cordis");
    }

    #[test]
    fn patch_writes_the_selected_agent_preset() {
        let yml = render_patch(None, Some("minimal"));
        assert!(yml.contains("default: minimal"));
        assert!(!yml.contains("default: standard"));
    }

    #[test]
    fn patch_writes_reasoning_effort_when_chosen() {
        let yml = render_patch(Some("max"), None);
        assert!(yml.contains("- id: llm-deepseek"));
        assert!(yml.contains("reasoningEffort: max"));
    }

    /// `default` 是 Kivio 侧的「不指定」哨兵，**不是** dsh 的合法档位值
    /// （schema 只认 `off|high|max`）。写进去整棵配置树会在启动时被拒。
    #[test]
    fn patch_omits_reasoning_for_default_and_blank() {
        for value in [None, Some(""), Some("   "), Some("default")] {
            let yml = render_patch(value, None);
            assert!(
                !yml.contains("reasoningEffort"),
                "reasoning={value:?} 不该写档位"
            );
            assert!(!yml.contains("llm-deepseek"));
        }
    }

    /// `off` 是合法档位（关思考），不能被当成「不指定」吞掉。
    #[test]
    fn off_is_a_real_effort_not_an_absence() {
        assert!(render_patch(Some("off"), None).contains("reasoningEffort: off"));
    }

    /// 未知档位（含换行 / YAML 注入）必须整段省略，不能原样插进共享 patch。
    #[test]
    fn unknown_reasoning_is_omitted_not_interpolated() {
        for value in [
            Some("HIGH"),
            Some("low"),
            Some("none"),
            Some("high\n- id: injected-tool\n  disabled: false"),
            Some("off\n- insert:\n    - id: evil"),
        ] {
            let yml = render_patch(value, None);
            assert!(
                !yml.contains("reasoningEffort"),
                "reasoning={value:?} 不该写档位"
            );
            assert!(
                !yml.contains("injected-tool"),
                "注入条目不得进 patch：{value:?}"
            );
            assert!(!yml.contains("evil"), "注入条目不得进 patch：{value:?}");
        }
    }

    /// patch 必须是**顶层 YAML 数组**（上游对这个文件的要求就是 "a top-level YAML array of
    /// patch entries"，空文件或映射都会 fail loud）。仓库没有 YAML 解析依赖，所以这里做
    /// 结构检查而不是真解析：每个非空非注释的顶层行，要么是 `- ` 开头的条目，要么是缩进的
    /// 续行 —— 出现顶层 `key:` 就说明写成映射了。
    #[test]
    fn patch_is_a_top_level_sequence() {
        for reasoning in [None, Some("high")] {
            let yml = render_patch(reasoning, None);
            let mut entries = 0;
            for line in yml.lines() {
                if line.trim().is_empty() || line.trim_start().starts_with('#') {
                    continue;
                }
                if line.starts_with("- ") {
                    entries += 1;
                    continue;
                }
                assert!(
                    line.starts_with(' '),
                    "顶层只能是数组条目或其缩进续行，出现了 {line:?}（reasoning={reasoning:?}）"
                );
            }
            assert!(
                entries >= 3,
                "至少 hmr / title-llm / insert 三条，实际 {entries}"
            );
        }
    }

    #[test]
    fn profile_dir_lives_under_dsh_home() {
        if let Some(dir) = profile_dir() {
            let shown = dir.to_string_lossy().replace('\\', "/");
            assert!(shown.ends_with("profiles/kivio"), "got {shown}");
        }
    }

    #[test]
    fn windows_pnpm_not_on_path_is_recognized_even_when_cmd_garbles_the_rest() {
        let real = "dsh: initialized profile kivio at C:\\Users\\18758\\.dsh\\profiles\\kivio\n\
                    'pnpm' \u{fffd}\u{fffd}\u{fffd}\n\
                    dsh: pnpm failed in profile directory C:\\Users\\18758\\.dsh\\profiles\\kivio";
        assert!(pnpm_missing_in_output(None, real));
        assert!(pnpm_missing_in_output(Some(127), "pnpm not found on PATH"));
        assert!(!pnpm_missing_in_output(Some(1), "network timeout contacting registry"));
    }

    #[test]
    fn profile_npmrc_pins_official_registry() {
        assert!(PROFILE_NPMRC.contains("registry=https://registry.npmjs.org/"));
        assert!(PROFILE_NPMRC.contains("@deepseek-ai:registry=https://registry.npmjs.org/"));
    }
}
