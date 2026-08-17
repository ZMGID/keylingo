use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::external_agents::registry::AGENT_DEFS;
use crate::external_agents::session::acp::detect_acp_models;
use crate::external_agents::session::claude_init::detect_claude_models;
use crate::external_agents::session::codex_app_server::{
    codex_static_fallback_probe, detect_codex_models, merge_codex_model_catalog,
    parse_codex_model_list_result,
};
use crate::external_agents::session::pi_rpc::parse_pi_models;
use crate::external_agents::types::{
    default_model_option, fallback_models_from_pairs, reasoning_options_from_pairs, DetectedAgent,
    ModelProbeStrategy, ModelSource, NativeProviderSummary, RuntimeAgentDef, RuntimeModelOption,
};
use crate::proc::NoConsoleWindow;

pub const EXTERNAL_AGENT_MODELS_CACHE_TTL: Duration = Duration::from_secs(300);
/// fallback（探测失败降级）结果的短负缓存 TTL：防止用户反复打开下拉连续触发 15s 探测风暴，
/// 又能在登录/网络恢复后较快重探。force 刷新绕过。
pub const EXTERNAL_AGENT_MODELS_FALLBACK_TTL: Duration = Duration::from_secs(30);

/// 可用性缓存与 cwd 无关（binary/version/auth 都不随目录变），用全局常量 key + 长 TTL。
/// 换会话直接命中，不再重测。手动刷新（force）绕过。
pub const AVAILABILITY_CACHE_KEY: &str = "__availability__";
pub const AVAILABILITY_CACHE_TTL: Duration = Duration::from_secs(600);

/// 只探可用性（binary + version + auth），**不跑昂贵的模型探测**（claude 达 25s）。
/// models 回填 `fallback_models`——列表阶段不展示真实模型，选中后由模型层懒查覆盖。
pub async fn detect_availability_single(def: &RuntimeAgentDef) -> DetectedAgent {
    let path = super::spawn::resolve_binary(def).await;
    let available = path.is_some();
    let version = if available {
        probe_version(def, path.as_deref()).await
    } else {
        None
    };
    let auth_status = if available {
        probe_auth(def, path.as_deref()).await
    } else {
        Some("unavailable".to_string())
    };
    DetectedAgent {
        id: def.id.to_string(),
        name: def.name.to_string(),
        available,
        path: path.map(|p| p.to_string_lossy().into_owned()),
        version,
        models: fallback_models_from_pairs(def.fallback_models),
        reasoning_options: reasoning_options_from_pairs(def.reasoning_options),
        sandbox_options: sandbox_options_for(def.id),
        auth_status,
        native_providers: native_provider_summaries(def.id),
        disabled: false,
        supports_steering: def.supports_steering,
        supports_follow_up: def.supports_follow_up,
    }
}

/// 并发探测所有 CLI 的可用性（cwd 无关）。
pub async fn detect_availability_all() -> Vec<DetectedAgent> {
    let handles: Vec<_> = AGENT_DEFS
        .iter()
        .map(|def| tokio::spawn(async move { detect_availability_single(def).await }))
        .collect();
    let mut out = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(agent) => out.push(agent),
            // B4：join 失败（探测 task panic）不再静默吞掉——记录以便定位。
            Err(err) => eprintln!("[external-agent] availability probe task join failed: {err}"),
        }
    }
    out
}

/// 单个 agent 的模型探测结果：模型列表 + reasoning 选项 + 来源 + 失败摘要，以及 CLI 自己当前
/// 配置的模型/推理等级（current_*，用于胶囊回填「同步 CLI 当前配置」）。current_* 拿不到 → None
/// → 前端显示「自动」。
pub struct AgentModelsResult {
    pub models: Vec<RuntimeModelOption>,
    pub reasoning_options: Vec<RuntimeModelOption>,
    pub source: ModelSource,
    pub probe_error: Option<String>,
    pub current_model: Option<String>,
    pub current_reasoning: Option<String>,
    /// 按模型区分的 effort 档位（kimi：K3 有 low/high/max，K2.7 always_thinking 无）。
    /// 空 = 不按模型区分，前端只用 `reasoning_options`。
    pub reasoning_by_model: std::collections::HashMap<String, Vec<RuntimeModelOption>>,
}

/// 只探单个 agent 的模型（cwd-scoped），供懒查命令用。返回模型、reasoning 选项，以及来源
/// （probed=真实探测 / fallback=探测失败降级静态表）、失败摘要与 CLI 当前配置。
pub async fn detect_agent_models(def: &RuntimeAgentDef, cwd: &Path) -> AgentModelsResult {
    let reasoning = reasoning_options_from_pairs(def.reasoning_options);
    let path = super::spawn::resolve_binary(def).await;
    let Some(_) = path.as_ref() else {
        return AgentModelsResult {
            models: fallback_models_from_pairs(def.fallback_models),
            reasoning_options: reasoning,
            source: ModelSource::Fallback,
            probe_error: Some("CLI 未安装或不在 PATH".to_string()),
            current_model: None,
            current_reasoning: None,
            reasoning_by_model: std::collections::HashMap::new(),
        };
    };
    match probe_models(def, path.as_deref(), cwd).await {
        Ok(probe) => {
            let models = probe.models;
            let mut current_model = probe.current_model;
            let mut current_reasoning = probe.current_reasoning;
            let probed_reasoning = probe.reasoning_options;
            let mut reasoning_by_model = probe.reasoning_by_model;
            // codex 的当前模型/推理不来自 model/list，而是读 config.toml 顶层键。
            if def.id == "codex" {
                let (cm, cr) = read_codex_current_config();
                current_model = current_model.or(cm);
                current_reasoning = current_reasoning.or(cr);
            } else if def.id == "pi" {
                // 不传 --thinking 时 pi 用 settings.json 的 defaultThinkingLevel；不读胶囊会恒显示「自动」。
                let (cm, cr) = read_pi_current_config();
                current_model = current_model.or(cm);
                current_reasoning = current_reasoning.or(cr);
            } else if def.id == "kimi" {
                // kimi 走 ACP 但 session/new 常不上报 currentModelId → 降级读 config.toml。
                // ACP 对 always_thinking 模型只给 options=[{on}]（已在 extract 里滤掉）；
                // 真实 low/high/max 在 config 的 per-model support_efforts 里。
                let (cm, cr) = read_kimi_current_config();
                if current_model.is_none() {
                    current_model = cm;
                }
                current_reasoning = current_reasoning.or(cr);
                let efforts_map = read_kimi_model_efforts();
                for (model_id, (opts, default_effort)) in efforts_map {
                    if current_reasoning.is_none()
                        && current_model.as_deref() == Some(model_id.as_str())
                    {
                        current_reasoning = default_effort;
                    }
                    reasoning_by_model.insert(model_id, opts);
                }
            } else if def.id == "claude" {
                // claude 的 system/init 只报模型、不报推理档位 → 读 settings.json 的
                // `effortLevel`（或 CLAUDE_EFFORT 环境变量）。此前漏了这条，胶囊上
                // 恒显示「自动」，哪怕用户明明配了 effortLevel: "high"。
                current_reasoning = current_reasoning
                    .or_else(crate::external_agents::session::claude_init::claude_config_effort);
            }
            // CLI 自报的多档优先；空（或已被滤掉的 On-only）回落 def 静态表。
            let mut reasoning_options = if probed_reasoning.is_empty() {
                reasoning
            } else {
                probed_reasoning
            };
            // kimi / codex：当前模型若有 per-model effort 表，用该表作为全局 reasoning_options
            // （胶囊默认档位列表）。kimi 空列表 = always_thinking；codex 无表则保留探测默认模型档。
            if matches!(def.id, "kimi" | "codex") {
                if let Some(model) = current_model.as_deref() {
                    if let Some(opts) = reasoning_by_model.get(model) {
                        reasoning_options = opts.clone();
                    }
                }
            }
            // 当前档位若不在可选列表里（比如残留的 "on"），清掉以免胶囊显示 On。
            if let Some(cur) = current_reasoning.as_deref() {
                let known = reasoning_options.iter().any(|o| o.id == cur)
                    || reasoning_by_model
                        .values()
                        .any(|opts| opts.iter().any(|o| o.id == cur));
                if !known
                    && matches!(
                        cur.to_lowercase().as_str(),
                        "on" | "off" | "true" | "false" | "enabled" | "disabled"
                    )
                {
                    current_reasoning = None;
                }
            }
            AgentModelsResult {
                models,
                reasoning_options,
                source: ModelSource::Probed,
                probe_error: None,
                current_model,
                current_reasoning,
                reasoning_by_model,
            }
        }
        Err(err) => {
            let models = fallback_models_from_pairs(def.fallback_models);
            // codex fallback：给每个真实模型挂上静态 effort，前端换模型时仍有档位可选。
            let reasoning_by_model = if def.id == "codex" {
                models
                    .iter()
                    .filter(|m| m.id != "default")
                    .map(|m| (m.id.clone(), reasoning.clone()))
                    .collect()
            } else {
                HashMap::new()
            };
            AgentModelsResult {
                models,
                reasoning_options: reasoning,
                source: ModelSource::Fallback,
                probe_error: Some(err),
                current_model: None,
                current_reasoning: None,
                reasoning_by_model,
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct DshSettings {
    #[serde(rename = "agent-default-model")]
    agent_default_model: Option<DshDefaultModel>,
    #[serde(rename = "api-gateway")]
    api_gateway: Option<DshDefaultModel>,
    #[serde(rename = "llm-deepseek")]
    llm_deepseek: Option<DshDeepseekSettings>,
    #[serde(rename = "llm-pi-ai")]
    llm_pi_ai: Option<DshPiAiSettings>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DshDefaultModel {
    provider: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DshDeepseekSettings {
    /// `None` = 适配器默认 flash/pro；`Some([])` = 用户明确不公布任何模型。
    models: Option<Vec<DshModelEntry>>,
    reasoning_effort: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DshPiAiSettings {
    #[serde(default)]
    providers: HashMap<String, DshProviderSettings>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DshProviderSettings {
    display_name: Option<String>,
    #[serde(alias = "baseURL")]
    base_url: Option<String>,
    api: Option<String>,
    #[serde(default)]
    models: Vec<DshModelEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DshModelEntry {
    Id(String),
    Detail(DshModelDetail),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DshModelDetail {
    id: String,
    name: Option<String>,
    context_window: Option<u32>,
    #[serde(default, rename = "reasoningEfforts")]
    reasoning_efforts: Option<serde_yaml::Value>,
}

impl DshModelEntry {
    fn parts(&self) -> Option<(&str, &str, Option<u32>)> {
        match self {
            DshModelEntry::Id(id) => {
                let id = id.trim();
                (!id.is_empty()).then_some((id, id, None))
            }
            DshModelEntry::Detail(detail) => {
                let id = detail.id.trim();
                if id.is_empty() {
                    return None;
                }
                let name = detail
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(id);
                Some((id, name, detail.context_window))
            }
        }
    }

    fn reasoning_efforts(&self) -> Option<&serde_yaml::Value> {
        match self {
            DshModelEntry::Id(_) => None,
            DshModelEntry::Detail(detail) => detail.reasoning_efforts.as_ref(),
        }
    }
}

const DSH_OFFICIAL_PROVIDER_ID: &str = "deepseek-official";
const DSH_OFFICIAL_PROVIDER_NAME: &str = "DeepSeek";
const DSH_OFFICIAL_DEFAULT_MODEL_COUNT: usize = 2;

/// 设置页「所有供应商」用的摘要。dsh 的官方 DeepSeek 不在 `llm-pi-ai` 里，
/// 但官方 UI 会单独列它；缓存命中时也要重读，所以 `pub(crate)`。
pub(crate) fn native_provider_summaries(agent_id: &str) -> Vec<NativeProviderSummary> {
    if agent_id != "dsh" {
        return Vec::new();
    }
    let text = dsh_settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    parse_dsh_native_provider_summaries(&text).unwrap_or_else(|_| {
        vec![official_deepseek_summary(
            DSH_OFFICIAL_DEFAULT_MODEL_COUNT,
            true,
        )]
    })
}

fn official_deepseek_summary(model_count: usize, is_default: bool) -> NativeProviderSummary {
    NativeProviderSummary {
        id: DSH_OFFICIAL_PROVIDER_ID.to_string(),
        name: DSH_OFFICIAL_PROVIDER_NAME.to_string(),
        base_url: None,
        api: None,
        model_count,
        is_default,
    }
}

fn parse_dsh_native_provider_summaries(text: &str) -> Result<Vec<NativeProviderSummary>, String> {
    let text = if text.trim().is_empty() { "{}" } else { text };
    let settings: DshSettings =
        serde_yaml::from_str(text).map_err(|err| format!("解析 dsh settings.yaml 失败：{err}"))?;
    let selected = settings
        .agent_default_model
        .as_ref()
        .or(settings.api_gateway.as_ref());
    let default_provider = selected
        .and_then(|selection| selection.provider.as_deref())
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .unwrap_or(DSH_OFFICIAL_PROVIDER_ID);
    let deepseek_model_count = match settings
        .llm_deepseek
        .as_ref()
        .and_then(|section| section.models.as_ref())
    {
        Some(entries) => entries
            .iter()
            .filter(|model| model.parts().is_some())
            .count(),
        None => DSH_OFFICIAL_DEFAULT_MODEL_COUNT,
    };
    let mut providers = vec![official_deepseek_summary(
        deepseek_model_count,
        default_provider == DSH_OFFICIAL_PROVIDER_ID,
    )];
    let mut extras: Vec<_> = settings
        .llm_pi_ai
        .map(|section| section.providers.into_iter().collect())
        .unwrap_or_default();
    extras.sort_by(|(a, _), (b, _)| a.cmp(b));
    providers.extend(extras.into_iter().map(|(id, config)| {
        NativeProviderSummary {
            is_default: default_provider == id.as_str(),
            name: config
                .display_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| id.clone()),
            base_url: config
                .base_url
                .map(|url| url.trim().to_string())
                .filter(|url| !url.is_empty()),
            api: config
                .api
                .map(|api| api.trim().to_string())
                .filter(|api| !api.is_empty()),
            model_count: config
                .models
                .iter()
                .filter(|model| model.parts().is_some())
                .count(),
            id,
        }
    }));
    Ok(providers)
}

fn dsh_settings_path() -> Option<std::path::PathBuf> {
    if let Some(home) = std::env::var_os("DSH_HOME") {
        let path = std::path::PathBuf::from(home);
        if !path.as_os_str().is_empty() {
            return Some(path.join("settings.yaml"));
        }
    }
    directories::BaseDirs::new().map(|base| base.home_dir().join(".dsh").join("settings.yaml"))
}

fn read_dsh_settings_models() -> Result<ProbeModelsOutput, String> {
    let path = dsh_settings_path().ok_or_else(|| "无法定位 dsh settings.yaml".to_string())?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 dsh 设置失败（{}）：{e}", path.display()))?;
    parse_dsh_settings_models(&text)
}

const DSH_PI_EFFORT_ORDER: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

fn dsh_effort_option(id: &str) -> RuntimeModelOption {
    RuntimeModelOption {
        id: id.to_string(),
        label: match id {
            "default" => "Default",
            "off" => "Off",
            "minimal" => "Minimal",
            "low" => "Low",
            "medium" => "Medium",
            "high" => "High",
            "xhigh" => "XHigh",
            "max" => "Max",
            other => other,
        }
        .to_string(),
        context_window_tokens: None,
    }
}

fn dsh_official_reasoning_options() -> Vec<RuntimeModelOption> {
    ["default", "off", "high", "max"]
        .into_iter()
        .map(dsh_effort_option)
        .collect()
}

fn dsh_reasoning_options_from_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Vec<RuntimeModelOption> {
    let set: HashSet<&str> = keys
        .into_iter()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect();
    let mut options = vec![dsh_effort_option("default")];
    options.extend(
        DSH_PI_EFFORT_ORDER
            .iter()
            .copied()
            .filter(|id| set.contains(id))
            .map(dsh_effort_option),
    );
    if options.len() == 1 {
        Vec::new()
    } else {
        options
    }
}

fn dsh_reasoning_options_from_yaml(
    value: Option<&serde_yaml::Value>,
) -> Option<Vec<RuntimeModelOption>> {
    match value {
        None => None,
        Some(serde_yaml::Value::Bool(false)) => Some(Vec::new()),
        Some(serde_yaml::Value::Mapping(map)) => {
            let keys: Vec<&str> = map.keys().filter_map(serde_yaml::Value::as_str).collect();
            Some(dsh_reasoning_options_from_keys(keys))
        }
        _ => Some(Vec::new()),
    }
}

fn dsh_reasoning_options_from_json(
    value: Option<&serde_json::Value>,
) -> Option<Vec<RuntimeModelOption>> {
    match value {
        None => None,
        Some(serde_json::Value::Bool(false)) => Some(Vec::new()),
        Some(serde_json::Value::Object(map)) => Some(dsh_reasoning_options_from_keys(
            map.keys().map(String::as_str),
        )),
        _ => Some(Vec::new()),
    }
}

fn parse_dsh_settings_models(text: &str) -> Result<ProbeModelsOutput, String> {
    let settings: DshSettings =
        serde_yaml::from_str(text).map_err(|e| format!("解析 dsh settings.yaml 失败：{e}"))?;
    let mut models = vec![default_model_option()];
    let mut seen = std::collections::HashSet::from(["default".to_string()]);
    let mut reasoning_by_model = HashMap::new();

    let deepseek_defaults = [
        ("deepseek-v4-flash", "DeepSeek-V4-Flash", Some(1_000_000)),
        ("deepseek-v4-pro", "DeepSeek-V4-Pro", Some(1_000_000)),
    ];
    match settings
        .llm_deepseek
        .as_ref()
        .and_then(|section| section.models.as_ref())
    {
        Some(entries) => {
            for entry in entries {
                if let Some((id, name, window)) = entry.parts() {
                    push_dsh_model(&mut models, &mut seen, id, name, window);
                    reasoning_by_model.insert(id.to_string(), dsh_official_reasoning_options());
                }
            }
        }
        None => {
            for (id, name, window) in deepseek_defaults {
                push_dsh_model(&mut models, &mut seen, id, name, window);
                reasoning_by_model.insert(id.to_string(), dsh_official_reasoning_options());
            }
        }
    }

    if let Some(pi_ai) = settings.llm_pi_ai.as_ref() {
        let mut providers: Vec<_> = pi_ai.providers.iter().collect();
        providers.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (provider, config) in providers {
            let provider_label = config
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or(provider);
            for entry in &config.models {
                let Some((id, name, window)) = entry.parts() else {
                    continue;
                };
                let wire_id = format!("{provider}:{id}");
                let label = format!("{name} ({provider_label})");
                push_dsh_model(&mut models, &mut seen, &wire_id, &label, window);
                reasoning_by_model.insert(
                    wire_id,
                    dsh_reasoning_options_from_yaml(entry.reasoning_efforts()).unwrap_or_default(),
                );
            }
        }
    }

    let selected = settings
        .agent_default_model
        .as_ref()
        .or(settings.api_gateway.as_ref());
    let mut current_model = selected.and_then(|selection| {
        let model = selection.model.as_deref()?.trim();
        if model.is_empty() {
            return None;
        }
        let provider = selection.provider.as_deref().unwrap_or("deepseek-official");
        Some(if provider == "deepseek-official" {
            model.to_string()
        } else {
            format!("{provider}:{model}")
        })
    });
    let mut current_reasoning = selected
        .and_then(|selection| selection.reasoning_effort.clone())
        .or_else(|| {
            settings
                .llm_deepseek
                .as_ref()
                .and_then(|section| section.reasoning_effort.clone())
        });

    if let Some(config) = crate::external_agents::overrides::agent_config("dsh") {
        for provider in config
            .providers
            .iter()
            .filter(|provider| !provider.disabled)
        {
            merge_kivio_dsh_provider(&mut models, &mut seen, &mut reasoning_by_model, provider)?;
        }
        if let Some(provider) = config
            .providers
            .iter()
            .find(|provider| provider.id == config.current_provider && !provider.disabled)
        {
            let route = provider.native_provider_id.trim();
            let model = provider.default_model.trim();
            if !route.is_empty() && !model.is_empty() {
                current_model = Some(format!("{route}:{model}"));
            }
        }
    }

    // 与 `dsh_jsonrpc::resolve_model_route_for_turn` / 适配器 `defaultEffort: high` 对齐：
    // settings.yaml 只有 onboarding、用户也没显式选时，轮次仍会跑 Flash + high。
    // 不报 current_* 的话前端只能显示 Auto，和实际在用的对不上。
    if current_model.is_none() {
        current_model = Some("deepseek-v4-flash".to_string());
    }
    if current_reasoning
        .as_deref()
        .map(str::trim)
        .is_none_or(|value| value.is_empty() || value == "default")
    {
        current_reasoning = Some("high".to_string());
    }

    Ok(probe_ok(
        models,
        current_model,
        current_reasoning,
        Vec::new(),
        reasoning_by_model,
    ))
}

fn push_dsh_model(
    models: &mut Vec<RuntimeModelOption>,
    seen: &mut std::collections::HashSet<String>,
    id: &str,
    label: &str,
    context_window_tokens: Option<u32>,
) {
    if !seen.insert(id.to_string()) {
        return;
    }
    models.push(RuntimeModelOption {
        id: id.to_string(),
        label: label.to_string(),
        context_window_tokens,
    });
}

fn merge_kivio_dsh_provider(
    models: &mut Vec<RuntimeModelOption>,
    seen: &mut std::collections::HashSet<String>,
    reasoning_by_model: &mut HashMap<String, Vec<RuntimeModelOption>>,
    provider: &crate::settings::ExternalCliProvider,
) -> Result<(), String> {
    let route = provider.native_provider_id.trim();
    if route.is_empty() {
        return Err(format!("dsh 供应商 {} 缺少原生供应商 ID", provider.name));
    }
    let config: serde_json::Value = serde_json::from_str(&provider.config_json)
        .map_err(|err| format!("解析 dsh 供应商 {} 失败：{err}", provider.name))?;
    let display_name = config
        .get("displayName")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(provider.name.as_str());
    let entries = config
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("dsh 供应商 {} 缺少模型列表", provider.name))?;
    for entry in entries {
        let Some(id) = entry
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let name = entry
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(id);
        let window = entry
            .get("contextWindow")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let wire_id = format!("{route}:{id}");
        let label = format!("{name} ({display_name})");
        push_dsh_model(models, seen, &wire_id, &label, window);
        if let Some(options) = dsh_reasoning_options_from_json(entry.get("reasoningEfforts")) {
            reasoning_by_model.insert(wire_id, options);
        } else {
            reasoning_by_model.entry(wire_id).or_default();
        }
    }
    Ok(())
}

/// 读 codex 当前配置：`~/.codex/config.toml` 顶层 `model` 与 `model_reasoning_effort`。手写扫描
/// 顶层 `key = "value"` 行（遇首个 `[section]` 即停），无 toml 依赖。缺文件/键 → None。
///
/// 选了第三方供应商时读的是那份私有 `CODEX_HOME` 里的 config.toml —— 子进程读哪份，这里就
/// 得读哪份，否则胶囊显示的当前模型是用户全局配置里的、和实际跑的对不上。
fn read_codex_current_config() -> (Option<String>, Option<String>) {
    let path =
        match crate::external_agents::provider_profile::provider_env("codex").get("CODEX_HOME") {
            Some(home) => std::path::PathBuf::from(home).join("config.toml"),
            None => {
                let Some(base) = directories::BaseDirs::new() else {
                    return (None, None);
                };
                base.home_dir().join(".codex").join("config.toml")
            }
        };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_codex_config_toplevel(&text),
        Err(_) => (None, None),
    }
}

/// 从 config.toml 文本抽取顶层 `model` / `model_reasoning_effort`。只认第一个 `[section]` 之前的
/// 顶层键，避免误取某个 profile/section 下的同名键。
fn parse_codex_config_toplevel(text: &str) -> (Option<String>, Option<String>) {
    let mut model = None;
    let mut reasoning = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            break; // 进入 section 表头，顶层键区结束
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "model" if model.is_none() => model = unquote_toml_scalar(value),
            "model_reasoning_effort" if reasoning.is_none() => {
                reasoning = unquote_toml_scalar(value)
            }
            _ => {}
        }
    }
    (model, reasoning)
}

/// 解析一个 TOML 标量右值：去引号（`"..."` / `'...'`），或裸值截到行内 `#` 注释。空 → None。
fn unquote_toml_scalar(value: &str) -> Option<String> {
    let v = value.trim();
    let out = if let Some(rest) = v.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else if let Some(rest) = v.strip_prefix('\'') {
        rest.split('\'').next().unwrap_or("")
    } else {
        v.split('#').next().unwrap_or("").trim()
    };
    let out = out.trim();
    if out.is_empty() {
        None
    } else {
        Some(out.to_string())
    }
}

/// 读 pi settings.json 的当前模型与默认思考档。缺文件/键 → None。
fn read_pi_current_config() -> (Option<String>, Option<String>) {
    let Some(base) = directories::BaseDirs::new() else {
        return (None, None);
    };
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| base.home_dir().join(".pi").join("agent"));
    let path = agent_dir.join("settings.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_pi_config(&text),
        Err(_) => (None, None),
    }
}

/// 从 pi settings.json 抽取 `(当前模型 id, 默认思考档)`。思考档只认 off..max，杂值丢弃。
fn parse_pi_config(text: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return (None, None);
    };
    let str_field = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let model = str_field("defaultModel").and_then(|model| match str_field("defaultProvider") {
        Some(provider) => Some(format!("{provider}/{model}")),
        None if model.contains('/') => Some(model.to_string()),
        None => None,
    });
    let reasoning = str_field("defaultThinkingLevel")
        .map(str::to_lowercase)
        .filter(|level| {
            matches!(
                level.as_str(),
                "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
            )
        });
    (model, reasoning)
}

/// 读 kimi 当前配置：`~/.kimi-code/config.toml`。ACP 探测无 currentModelId 时降级用此。
/// 缺文件/键 → None。
fn read_kimi_current_config() -> (Option<String>, Option<String>) {
    let Some(base) = directories::BaseDirs::new() else {
        return (None, None);
    };
    let path = base.home_dir().join(".kimi-code").join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_kimi_config(&text),
        Err(_) => (None, None),
    }
}

/// 从 kimi config.toml 文本抽取 `(default_model, thinking_effort)`。`default_model` 取顶层键；
/// `thinking_effort` 取 `[thinking]` section 内 `effort`，且仅当同 section `enabled = true` 时给出。
/// 手写 section-aware 扫描（无 toml 依赖）——codex 顶层扫描器遇 section 即停，无法读进 section。
fn parse_kimi_config(text: &str) -> (Option<String>, Option<String>) {
    let mut default_model = None;
    let mut section: Option<String> = None; // None = 顶层
    let mut thinking_enabled = false;
    let mut thinking_effort: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            section = Some(rest.split(']').next().unwrap_or("").trim().to_string());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        match section.as_deref() {
            None if key == "default_model" && default_model.is_none() => {
                default_model = unquote_toml_scalar(value);
            }
            Some("thinking") => match key {
                "enabled" => {
                    thinking_enabled = unquote_toml_scalar(value).as_deref() == Some("true")
                }
                "effort" if thinking_effort.is_none() => {
                    thinking_effort = unquote_toml_scalar(value)
                }
                _ => {}
            },
            _ => {}
        }
    }
    let reasoning = if thinking_enabled {
        thinking_effort
    } else {
        None
    };
    (default_model, reasoning)
}

/// 解析 kimi config 里每个模型的 `support_efforts` / `default_effort`。
/// always_thinking 模型（K2.7）没有 support_efforts → 不进 map → 前端不显示 effort 胶囊。
/// K3 有 `support_efforts = ["low","high","max"]` → 按模型给出真实档位。
fn parse_kimi_model_efforts(
    text: &str,
) -> std::collections::HashMap<String, (Vec<RuntimeModelOption>, Option<String>)> {
    let mut out: std::collections::HashMap<String, (Vec<RuntimeModelOption>, Option<String>)> =
        std::collections::HashMap::new();
    let mut section: Option<String> = None;
    let mut efforts: Vec<String> = Vec::new();
    let mut default_effort: Option<String> = None;

    let flush = |section: &Option<String>,
                 efforts: &mut Vec<String>,
                 default_effort: &mut Option<String>,
                 out: &mut std::collections::HashMap<
        String,
        (Vec<RuntimeModelOption>, Option<String>),
    >| {
        let Some(sec) = section.as_deref() else {
            return;
        };
        // [models."kimi-code/k3"] 或 [models.kimi-code/k3]
        let Some(model_id) = sec
            .strip_prefix("models.")
            .map(|s| s.trim().trim_matches('"').to_string())
        else {
            return;
        };
        if efforts.is_empty() {
            // always_thinking 模型（K2.7 Coding）在 config 里就没有 support_efforts —— 这不是
            // 「没读到」，是「明确没有档位」。必须落一个空条目：前端 activeReasoningOptions 靠
            // **键是否存在**区分二者，键缺失会退回 agent 级列表，把 K3 的 low/high/max 显示
            // 在 K2.7 上（用户实测到的就是这个）。
            *default_effort = None;
            out.insert(model_id, (Vec::new(), None));
            return;
        }
        let opts: Vec<RuntimeModelOption> = efforts
            .iter()
            .map(|id| {
                let label = match id.as_str() {
                    "low" => "Low",
                    "medium" | "med" => "Medium",
                    "high" => "High",
                    "max" | "xhigh" => "Max",
                    other => other,
                };
                RuntimeModelOption {
                    id: id.clone(),
                    label: label.to_string(),
                    context_window_tokens: None,
                }
            })
            .collect();
        out.insert(model_id, (opts, default_effort.take()));
        efforts.clear();
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            flush(&section, &mut efforts, &mut default_effort, &mut out);
            section = Some(rest.split(']').next().unwrap_or("").trim().to_string());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !section.as_deref().is_some_and(|s| s.starts_with("models.")) {
            continue;
        }
        match key {
            "support_efforts" => {
                // support_efforts = [ "low", "high", "max" ]
                let body = value.trim().trim_start_matches('[').trim_end_matches(']');
                for part in body.split(',') {
                    if let Some(id) = unquote_toml_scalar(part) {
                        if !id.is_empty() {
                            efforts.push(id);
                        }
                    }
                }
            }
            "default_effort" if default_effort.is_none() => {
                default_effort = unquote_toml_scalar(value);
            }
            _ => {}
        }
    }
    flush(&section, &mut efforts, &mut default_effort, &mut out);
    out
}

fn read_kimi_model_efforts(
) -> std::collections::HashMap<String, (Vec<RuntimeModelOption>, Option<String>)> {
    let Some(base) = directories::BaseDirs::new() else {
        return std::collections::HashMap::new();
    };
    let path = base.home_dir().join(".kimi-code").join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_kimi_model_efforts(&text),
        Err(_) => std::collections::HashMap::new(),
    }
}

pub async fn detect_single_agent(def: &RuntimeAgentDef, cwd: &Path) -> DetectedAgent {
    let path = super::spawn::resolve_binary(def).await;
    let available = path.is_some();
    let version = if available {
        probe_version(def, path.as_deref()).await
    } else {
        None
    };
    let auth_status = if available {
        probe_auth(def, path.as_deref()).await
    } else {
        Some("unavailable".to_string())
    };
    let models = if available {
        probe_models(def, path.as_deref(), cwd)
            .await
            .map(|probe| probe.models)
            .unwrap_or_else(|_| fallback_models_from_pairs(def.fallback_models))
    } else {
        fallback_models_from_pairs(def.fallback_models)
    };

    DetectedAgent {
        id: def.id.to_string(),
        name: def.name.to_string(),
        available,
        path: path.map(|p| p.to_string_lossy().into_owned()),
        version,
        models,
        reasoning_options: reasoning_options_from_pairs(def.reasoning_options),
        sandbox_options: sandbox_options_for(def.id),
        auth_status,
        native_providers: native_provider_summaries(def.id),
        disabled: false,
        supports_steering: def.supports_steering,
        supports_follow_up: def.supports_follow_up,
    }
}

/// Sandbox/permission levels offered per agent. Ids are the agent's native flag values so
/// `build_args` can pass them straight through (claude `--permission-mode`, codex `--sandbox`).
/// Agents without a meaningful sandbox flag return an empty list (no capsule shown).
pub fn sandbox_options_for(agent_id: &str) -> Vec<RuntimeModelOption> {
    let pairs: &[(&str, &str)] = match agent_id {
        "claude" => &[
            ("plan", "计划 (只读)"),
            // `default` 档 = 写文件 / 跑命令前弹卡片问用户（走 stdio 控制通道的
            // `can_use_tool`，见 `defs::claude::claude_permission_prompt_args`）。
            // **不是默认选中项** —— 默认仍是下面的 `bypassPermissions`，否则既有用户的
            // 对话会突然开始弹卡片。
            ("default", "每次确认"),
            ("acceptEdits", "接受编辑"),
            // 官方 `--permission-mode` 的另外两档。缺了它们，用户只能在「每次都问」和
            // 「完全放行」之间跳，中间地带整个拿不到：
            // - `auto`：带分类器的自动模式，连拒 3 次 / 累计 20 次后自动回落到询问。
            // - `dontAsk`：只放行 permissions.allow 规则与只读命令集，其余一律拒、不打扰。
            //   官方为 headless 点名推荐的就是这一档。
            // （`manual` 是 `default` 的别名，不重复列。）
            ("auto", "自动"),
            ("dontAsk", "不打扰 (只放行安全操作)"),
            ("bypassPermissions", "完全 (默认)"),
        ],
        "codex" | "dsh" => &[
            ("read-only", "只读"),
            ("workspace-write", "工作区写 (默认)"),
            ("danger-full-access", "完全"),
        ],
        _ => &[],
    };
    pairs
        .iter()
        .map(|(id, label)| RuntimeModelOption {
            id: (*id).to_string(),
            label: (*label).to_string(),
            context_window_tokens: None,
        })
        .collect()
}

async fn probe_version(def: &RuntimeAgentDef, path: Option<&std::path::Path>) -> Option<String> {
    let bin = path?;
    let output = crate::external_agents::spawn::agent_cli_command(def, bin)
        .args(def.version_args)
        .no_console_window()
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

async fn probe_auth(def: &RuntimeAgentDef, path: Option<&std::path::Path>) -> Option<String> {
    let args = def.auth_probe_args?;
    let bin = path?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        crate::external_agents::spawn::agent_cli_command(def, bin)
            .args(args)
            .no_console_window()
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if output.status.success() {
        Some("ok".to_string())
    } else {
        Some("auth_required".to_string())
    }
}

/// 探测模型列表 + CLI 当前配置。
///
/// `current_*` 仅 ACP / claude 路径能从探测本身给出；codex 由上层从 config.toml 补齐。
/// `reasoning_options` / `reasoning_by_model`：CLI 自报档位（codex model/list、kimi ACP）；
/// 空 = 回落 `def.reasoning_options` 静态表。
struct ProbeModelsOutput {
    models: Vec<RuntimeModelOption>,
    current_model: Option<String>,
    current_reasoning: Option<String>,
    reasoning_options: Vec<RuntimeModelOption>,
    reasoning_by_model: HashMap<String, Vec<RuntimeModelOption>>,
}

fn probe_ok(
    models: Vec<RuntimeModelOption>,
    current_model: Option<String>,
    current_reasoning: Option<String>,
    reasoning_options: Vec<RuntimeModelOption>,
    reasoning_by_model: HashMap<String, Vec<RuntimeModelOption>>,
) -> ProbeModelsOutput {
    ProbeModelsOutput {
        models,
        current_model,
        current_reasoning,
        reasoning_options,
        reasoning_by_model,
    }
}

async fn probe_models(
    def: &RuntimeAgentDef,
    path: Option<&std::path::Path>,
    cwd: &Path,
) -> Result<ProbeModelsOutput, String> {
    let bin = path.ok_or_else(|| "CLI 可执行文件未定位".to_string())?;

    // dsh 的模型目录就在 `$DSH_HOME/settings.yaml`，结构化读文件比 boot 整棵 profile 快几秒，
    // 也不会为打开一个下拉框启动 agent / MCP / watcher。bin 只用于上面的已安装判定。
    if def.id == "dsh" {
        return read_dsh_settings_models();
    }

    // OpenCode's native command is the source of truth for merged global/project JSONC config.
    // Older versions without `models` fall through to ACP, then the static definition fallback.
    if def.id == "opencode" {
        let timeout_secs = def.list_models_timeout_secs.unwrap_or(15);
        if let Some(models) = probe_opencode_models(def, bin, cwd, timeout_secs).await {
            return Ok(probe_ok(models, None, None, Vec::new(), HashMap::new()));
        }
    }

    // Codex：对齐 desktop-cc-gui curated four；runtime 只 enrich 同 id。
    // list/debug 都失败时仍 Ok(curated)—— 下拉永不空（不再抛误导性 Err）。
    if def.id == "codex" {
        let timeout_secs = def.list_models_timeout_secs.unwrap_or(20);
        let (config_model, _) = read_codex_current_config();
        let runtime = if let Some(probe) = detect_codex_models(bin, cwd, timeout_secs).await {
            Some(probe)
        } else {
            probe_codex_debug_models(def, bin, cwd, timeout_secs).await
        };
        let base = runtime.unwrap_or_else(codex_static_fallback_probe);
        let merged = merge_codex_model_catalog(base, config_model.as_deref());
        return Ok(probe_ok(
            merged.models,
            None,
            None,
            merged.reasoning_options,
            merged.reasoning_by_model,
        ));
    }

    if def.model_probe == Some(ModelProbeStrategy::Acp) {
        let args: Vec<&str> = def
            .model_probe_args
            .ok_or_else(|| "缺少 ACP 模型探测参数".to_string())?
            .iter()
            .copied()
            .collect();
        let timeout_secs = def.list_models_timeout_secs.unwrap_or(15);
        return detect_acp_models(bin, &args, cwd, timeout_secs)
            .await
            .map(|probe| {
                probe_ok(
                    probe.models,
                    probe.current_model,
                    probe.current_reasoning,
                    probe.reasoning_options,
                    HashMap::new(),
                )
            })
            .ok_or_else(|| "ACP 模型探测未返回模型（可能未登录或握手失败）".to_string());
    }

    if def.model_probe == Some(ModelProbeStrategy::ClaudeInit) {
        let timeout_secs = def.list_models_timeout_secs.unwrap_or(25);
        return match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            detect_claude_models(bin, cwd),
        )
        .await
        {
            Ok(Some((models, current_model))) => Ok(probe_ok(
                models,
                current_model,
                None,
                Vec::new(),
                HashMap::new(),
            )),
            Ok(None) => Err("Claude 初始化未上报模型".to_string()),
            Err(_) => Err(format!("Claude 模型探测超时（{timeout_secs}s）")),
        };
    }

    let args = def
        .list_models_args
        .ok_or_else(|| "该 CLI 未配置列模型命令".to_string())?;
    let timeout_secs = def.list_models_timeout_secs.unwrap_or(5);
    let output = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        crate::external_agents::spawn::agent_cli_command(def, bin)
            .args(args)
            .current_dir(cwd)
            .no_console_window()
            .output(),
    )
    .await
    .map_err(|_| format!("列模型命令超时（{timeout_secs}s）"))?
    .map_err(|e| format!("列模型命令启动失败：{e}"))?;

    // Pi prints its model table to stdout (the `models_from_stderr` name is historical — older
    // builds used stderr). Prefer whichever stream actually has content, then parse the table.
    if def.models_from_stderr {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let text = if !stdout.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        return parse_pi_models(text.as_ref())
            .map(|models| {
                // `thinking no` 的模型给空档位表 → 前端隐藏 effort 胶囊（pi 对无思考
                // 模型的 get_available_thinking_levels 也只回 ["off"]，给档位是骗人）。
                // `thinking yes` 不建表，回落 def 静态档位（off..xhigh 全档）。
                let reasoning_by_model: HashMap<String, Vec<RuntimeModelOption>> =
                    crate::external_agents::session::pi_rpc::parse_pi_model_thinking(text.as_ref())
                        .into_iter()
                        .filter(|(_, supported)| !supported)
                        .map(|(id, _)| (id, Vec::new()))
                        .collect();
                probe_ok(models, None, None, Vec::new(), reasoning_by_model)
            })
            .ok_or_else(|| "未从 pi 输出解析出模型".to_string());
    }

    if !output.status.success() {
        return Err(format!(
            "列模型命令退出码非零：{}",
            output.status.code().unwrap_or(-1)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_models_list(def.id, text.as_ref())
        .map(|models| probe_ok(models, None, None, Vec::new(), HashMap::new()))
        .ok_or_else(|| "未从列模型输出解析出模型".to_string())
}

/// Secondary Codex catalog path: `codex debug models` JSON on stdout.
/// Shape differs from `model/list` (`slug` / `supported_reasoning_levels` / `context_window`),
/// so we normalize into the same `CodexModelsProbe` used by the app-server path.
async fn probe_codex_debug_models(
    def: &RuntimeAgentDef,
    bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
) -> Option<crate::external_agents::session::codex_app_server::CodexModelsProbe> {
    let output = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        crate::external_agents::spawn::agent_cli_command(def, bin)
            .args(["debug", "models"])
            .current_dir(cwd)
            .no_console_window()
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_codex_debug_models_json(text.as_ref())
}

/// Normalize `codex debug models` JSON into the model/list-shaped probe result.
fn parse_codex_debug_models_json(
    stdout: &str,
) -> Option<crate::external_agents::session::codex_app_server::CodexModelsProbe> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed.to_lowercase().contains("no models available") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    // debug models: `{ "models": [ { "slug", "display_name", "context_window",
    //   "supported_reasoning_levels": [{ "effort", "description" }], ... } ] }`
    // Reuse model/list parser by mapping fields into its accepted aliases.
    let models = value.get("models").and_then(|v| v.as_array())?;
    let mapped: Vec<serde_json::Value> = models
        .iter()
        .map(|entry| {
            let mut obj = serde_json::Map::new();
            if let Some(id) = entry.get("slug").or_else(|| entry.get("id")).cloned() {
                obj.insert("id".into(), id.clone());
                obj.insert("model".into(), id);
            }
            if let Some(name) = entry
                .get("display_name")
                .or_else(|| entry.get("displayName"))
                .cloned()
            {
                obj.insert("displayName".into(), name);
            }
            if let Some(desc) = entry.get("description").cloned() {
                obj.insert("description".into(), desc);
            }
            if let Some(cw) = entry
                .get("context_window")
                .or_else(|| entry.get("contextWindow"))
                .cloned()
            {
                obj.insert("context_window".into(), cw);
            }
            if let Some(levels) = entry
                .get("supported_reasoning_levels")
                .or_else(|| entry.get("supportedReasoningEfforts"))
                .cloned()
            {
                // Normalize effort key so parse_codex_reasoning_efforts accepts it.
                if let Some(arr) = levels.as_array() {
                    let norm: Vec<serde_json::Value> = arr
                        .iter()
                        .map(|level| {
                            let mut m = serde_json::Map::new();
                            if let Some(effort) = level
                                .get("effort")
                                .or_else(|| level.get("reasoningEffort"))
                                .or_else(|| level.get("reasoning_effort"))
                                .cloned()
                            {
                                m.insert("reasoningEffort".into(), effort);
                            }
                            if let Some(d) = level.get("description").cloned() {
                                m.insert("description".into(), d);
                            }
                            serde_json::Value::Object(m)
                        })
                        .collect();
                    obj.insert(
                        "supportedReasoningEfforts".into(),
                        serde_json::Value::Array(norm),
                    );
                }
            }
            if let Some(def) = entry
                .get("default_reasoning_level")
                .or_else(|| entry.get("defaultReasoningEffort"))
                .cloned()
            {
                obj.insert("defaultReasoningEffort".into(), def);
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    parse_codex_model_list_result(&serde_json::json!({ "data": mapped }))
}

async fn probe_opencode_models(
    def: &RuntimeAgentDef,
    bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
) -> Option<Vec<RuntimeModelOption>> {
    let output = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        crate::external_agents::spawn::agent_cli_command(def, bin)
            .arg("models")
            .current_dir(cwd)
            .no_console_window()
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_opencode_models(String::from_utf8_lossy(&output.stdout).as_ref())
}

fn parse_opencode_models(stdout: &str) -> Option<Vec<RuntimeModelOption>> {
    let mut out = vec![default_model_option()];
    let mut seen = std::collections::HashSet::from(["default".to_string()]);
    for line in stdout.lines() {
        let id = line.trim();
        if id.is_empty() || id.chars().any(char::is_whitespace) {
            continue;
        }
        let Some((provider, model)) = id.split_once('/') else {
            continue;
        };
        if provider.is_empty() || model.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        out.push(RuntimeModelOption {
            id: id.to_string(),
            label: id.to_string(),
            context_window_tokens: None,
        });
    }
    (out.len() > 1).then_some(out)
}

fn parse_models_list(agent_id: &str, stdout: &str) -> Option<Vec<RuntimeModelOption>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed.to_lowercase().contains("no models available") {
        return None;
    }
    let mut out = vec![default_model_option()];
    // 曾是多 CLI 的 match（kimi `provider list --json` 分支随 kimi 迁 ACP 删除）；现在只剩
    // codex 一家还走文本 list-models 探测。
    if agent_id == "codex" {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(models) = value.get("models").and_then(|v| v.as_array()) {
                for entry in models {
                    let id = entry
                        .get("slug")
                        .or_else(|| entry.get("id"))
                        .and_then(|v| v.as_str())?;
                    out.push(RuntimeModelOption {
                        id: id.to_string(),
                        label: id.to_string(),
                        // codex reports the real window per model (e.g. 272000); without
                        // it the context gauge falls back to the generic 200K estimate.
                        context_window_tokens: entry
                            .get("context_window")
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32),
                    });
                }
            }
        }
        // "Default" = codex picks its own default (the first listed model), so give the
        // synthetic entry that model's window instead of leaving it unknown.
        if out.len() > 1 {
            out[0].context_window_tokens = out[1].context_window_tokens;
        }
    }
    if out.len() > 1 {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires live pi CLI on PATH"]
    async fn live_pi_models_from_config_not_fallback() {
        use crate::external_agents::registry::get_agent_def;
        let def = get_agent_def("pi").expect("pi def");
        let detected = detect_single_agent(def, &std::env::temp_dir()).await;
        assert!(detected.available, "pi should be on PATH");
        for m in &detected.models {
            eprintln!("  {} -> {}", m.id, m.label);
        }
        // Real discovered models, not the bogus generic fallback.
        assert!(
            detected.models.iter().any(|m| m.id.contains('/')
                && !m.id.starts_with("anthropic/")
                && !m.id.starts_with("openai/")),
            "expected user-configured pi models, got: {:?}",
            detected.models.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_codex_debug_models_json_normalizes_to_catalog() {
        let probe = parse_codex_debug_models_json(
            r#"{
              "models": [
                {
                  "slug": "gpt-5.5",
                  "display_name": "GPT-5.5",
                  "context_window": 272000,
                  "default_reasoning_level": "medium",
                  "supported_reasoning_levels": [
                    {"effort": "low", "description": "Fast"},
                    {"effort": "medium", "description": "Balanced"},
                    {"effort": "high", "description": "Deep"}
                  ]
                },
                {"slug": "o3"}
              ]
            }"#,
        )
        .unwrap();
        assert!(probe.models.iter().any(|m| m.id == "gpt-5.5"));
        assert!(probe.models.iter().any(|m| m.id == "o3"));
        let sol = probe.models.iter().find(|m| m.id == "gpt-5.5").unwrap();
        assert_eq!(sol.context_window_tokens, Some(272000));
        assert_eq!(sol.label, "GPT-5.5");
        assert_eq!(probe.models[0].id, "default");
        assert_eq!(probe.models[0].context_window_tokens, Some(272000));
        // per-model efforts from supported_reasoning_levels
        let efforts = probe.reasoning_by_model.get("gpt-5.5").unwrap();
        assert!(efforts.iter().any(|e| e.id == "low"));
        assert!(efforts.iter().any(|e| e.id == "high"));
        assert!(probe.reasoning_options.iter().any(|e| e.id == "medium"));
    }

    #[test]
    fn parse_opencode_models_accepts_custom_providers_and_variants() {
        let models =
            parse_opencode_models("custom/minimax-m2.7\ncustom/deep/model-v1\nopenai/gpt-5\n")
                .unwrap();
        assert!(models.iter().any(|model| model.id == "custom/minimax-m2.7"));
        assert!(models
            .iter()
            .any(|model| model.id == "custom/deep/model-v1"));
        assert!(models.iter().any(|model| model.id == "openai/gpt-5"));
    }

    #[test]
    fn parse_opencode_models_ignores_invalid_and_duplicate_lines() {
        let models = parse_opencode_models(
            "custom/model-a\ncustom/model-a\ninvalid\n/providerless\nprovider/\nlog line here\n",
        )
        .unwrap();
        assert_eq!(
            models
                .iter()
                .filter(|model| model.id == "custom/model-a")
                .count(),
            1
        );
        assert_eq!(models.len(), 2, "default plus one valid custom model");
    }

    #[test]
    fn parse_opencode_models_returns_none_without_valid_models() {
        assert!(parse_opencode_models("").is_none());
        assert!(
            parse_opencode_models("invalid\n/providerless\nprovider/\nlog line here\n").is_none()
        );
    }

    #[test]
    fn codex_config_reads_toplevel_model_and_reasoning() {
        let (model, reasoning) = parse_codex_config_toplevel(
            "model = \"gpt-5.6-sol\"\nmodel_reasoning_effort = \"high\"\n",
        );
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn codex_config_missing_keys_are_none() {
        let (model, reasoning) =
            parse_codex_config_toplevel("# just a comment\napproval_policy = \"on-request\"\n");
        assert!(model.is_none());
        assert!(reasoning.is_none());
    }

    #[test]
    fn codex_config_ignores_keys_inside_sections() {
        // 顶层 model 才算数；[section] 之后的同名键（如 profile 覆盖）不应被误取。
        let text = "model = \"gpt-5.6-sol\"\n\n[profiles.fast]\nmodel = \"o3-mini\"\nmodel_reasoning_effort = \"low\"\n";
        let (model, reasoning) = parse_codex_config_toplevel(text);
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
        // 顶层没有 model_reasoning_effort（只在 section 里）→ None。
        assert!(reasoning.is_none());
    }

    #[test]
    fn pi_config_joins_provider_and_model() {
        let (model, reasoning) = parse_pi_config(
            "{\"defaultProvider\":\"edgefn\",\"defaultModel\":\"DeepSeek-V4-Flash\",\"defaultThinkingLevel\":\"high\"}",
        );
        assert_eq!(model.as_deref(), Some("edgefn/DeepSeek-V4-Flash"));
        assert_eq!(reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn pi_config_uses_model_alone_when_provider_missing() {
        // provider 缺失但 model 自身已含 `/` → 直接用。
        let (model, _) = parse_pi_config("{\"defaultModel\":\"edgefn/DeepSeek-V4-Flash\"}");
        assert_eq!(model.as_deref(), Some("edgefn/DeepSeek-V4-Flash"));
        // provider 缺失且 model 不含 `/` → None（无法拼出合法 id）。
        assert!(parse_pi_config("{\"defaultModel\":\"gpt\"}").0.is_none());
    }

    #[test]
    fn pi_config_missing_model_is_none() {
        assert!(parse_pi_config("{\"defaultProvider\":\"edgefn\"}")
            .0
            .is_none());
        assert!(parse_pi_config("{}").0.is_none());
        // 非法 JSON 也不 panic → None。
        assert!(parse_pi_config("not json").0.is_none());
    }

    #[test]
    fn pi_config_thinking_level_rejects_garbage_and_is_independent_of_model() {
        // 模型缺失不影响思考档（各自独立抽取）。
        let (model, reasoning) = parse_pi_config("{\"defaultThinkingLevel\":\"XHigh\"}");
        assert!(model.is_none());
        assert_eq!(reasoning.as_deref(), Some("xhigh"));
        // 不在 pi 档位表里的杂值丢弃（不能让胶囊显示垃圾字符串）。
        assert!(parse_pi_config("{\"defaultThinkingLevel\":\"turbo\"}")
            .1
            .is_none());
        assert!(parse_pi_config("{\"defaultThinkingLevel\":\"\"}")
            .1
            .is_none());
        // Kivio 表单会写 max，必须能读回来。
        assert_eq!(
            parse_pi_config("{\"defaultThinkingLevel\":\"Max\"}")
                .1
                .as_deref(),
            Some("max")
        );
    }

    #[test]
    fn kimi_model_efforts_from_support_efforts() {
        let text = r#"
default_model = "kimi-code/kimi-for-coding"

[models."kimi-code/kimi-for-coding"]
display_name = "K2.7 Coding"
capabilities = [ "thinking", "always_thinking" ]

[models."kimi-code/k3"]
display_name = "K3"
support_efforts = [ "low", "high", "max" ]
default_effort = "high"
"#;
        let map = parse_kimi_model_efforts(text);
        // 键必须在、且为空 —— 前端靠「键存在」判定「这个模型明确没有档位」；
        // 键缺失会被当成「没读到」而退回 agent 级列表（= K3 的 low/high/max）。
        let (opts, default) = map
            .get("kimi-code/kimi-for-coding")
            .expect("always_thinking 模型也要进 map");
        assert!(opts.is_empty(), "always_thinking 无档位: {opts:?}");
        assert!(default.is_none());
        let (opts, default) = map.get("kimi-code/k3").expect("k3 efforts");
        assert_eq!(
            opts.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            vec!["low", "high", "max"]
        );
        assert_eq!(default.as_deref(), Some("high"));
    }

    #[test]
    fn kimi_config_reads_default_model_and_thinking_effort() {
        let text = "default_model = \"kimi-code/kimi-for-coding\"\n\n[thinking]\nenabled = true\neffort = \"high\"\n";
        let (model, reasoning) = parse_kimi_config(text);
        assert_eq!(model.as_deref(), Some("kimi-code/kimi-for-coding"));
        assert_eq!(reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn kimi_config_effort_none_when_thinking_disabled() {
        // enabled=false → effort 不生效，reasoning=None；default_model 仍读到。
        let text =
            "default_model = \"kimi-code/kimi-for-coding\"\n[thinking]\nenabled = false\neffort = \"high\"\n";
        let (model, reasoning) = parse_kimi_config(text);
        assert_eq!(model.as_deref(), Some("kimi-code/kimi-for-coding"));
        assert!(reasoning.is_none());
    }

    #[test]
    fn kimi_config_respects_section_boundaries() {
        // default_model 只认顶层；[thinking] 之外的 effort 不算数，缺 default_model → None。
        let text = "[other]\ndefault_model = \"wrong\"\neffort = \"low\"\n\n[thinking]\nenabled = true\neffort = \"medium\"\n";
        let (model, reasoning) = parse_kimi_config(text);
        assert!(model.is_none());
        assert_eq!(reasoning.as_deref(), Some("medium"));
    }

    #[test]
    fn dsh_settings_exposes_deepseek_and_relay_models() {
        let result = parse_dsh_settings_models(
            r#"
agent-default-model:
  provider: openrouter
  model: deepseek/deepseek-r1
  reasoningEffort: max
llm-deepseek:
  models:
    - id: deepseek-v4-pro
      name: V4 Pro
      contextWindow: 131072
llm-pi-ai:
  providers:
    openrouter:
      displayName: OpenRouter
      models:
        - id: deepseek/deepseek-r1
          name: DeepSeek R1
          contextWindow: 163840
"#,
        )
        .expect("parse dsh settings");
        assert_eq!(
            result
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "default",
                "deepseek-v4-pro",
                "openrouter:deepseek/deepseek-r1"
            ]
        );
        assert_eq!(result.models[1].context_window_tokens, Some(131_072));
        assert_eq!(result.models[2].label, "DeepSeek R1 (OpenRouter)");
        assert_eq!(
            result.current_model.as_deref(),
            Some("openrouter:deepseek/deepseek-r1")
        );
        assert_eq!(result.current_reasoning.as_deref(), Some("max"));
    }

    #[test]
    fn dsh_settings_defaults_native_catalog_but_respects_explicit_empty_models() {
        let defaults = parse_dsh_settings_models("{}").expect("default dsh settings");
        assert_eq!(defaults.models.len(), 3);
        assert_eq!(defaults.models[1].id, "deepseek-v4-flash");
        assert_eq!(defaults.models[2].id, "deepseek-v4-pro");
        assert_eq!(defaults.current_model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(defaults.current_reasoning.as_deref(), Some("high"));

        let empty = parse_dsh_settings_models("llm-deepseek:\n  models: []\n")
            .expect("explicit empty catalog");
        assert_eq!(
            empty
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["default"]
        );
        assert_eq!(empty.current_model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(empty.current_reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn kivio_dsh_provider_models_are_namespaced_by_route() {
        let provider = crate::settings::ExternalCliProvider {
            name: "Relay".to_string(),
            native_provider_id: "relay-one".to_string(),
            config_json: serde_json::json!({
                "displayName": "Relay One",
                "models": [{
                    "id": "gpt-test",
                    "name": "GPT Test",
                    "contextWindow": 256000,
                    "reasoningEfforts": { "off": null, "low": "low", "high": "high" }
                }]
            })
            .to_string(),
            ..Default::default()
        };
        let mut models = vec![default_model_option()];
        let mut seen = std::collections::HashSet::from(["default".to_string()]);
        let mut reasoning_by_model = HashMap::new();
        merge_kivio_dsh_provider(&mut models, &mut seen, &mut reasoning_by_model, &provider)
            .unwrap();
        assert_eq!(models[1].id, "relay-one:gpt-test");
        assert_eq!(models[1].label, "GPT Test (Relay One)");
        assert_eq!(models[1].context_window_tokens, Some(256_000));
        assert_eq!(
            reasoning_by_model
                .get("relay-one:gpt-test")
                .map(|items| items
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>()),
            Some(vec!["default", "off", "low", "high"])
        );
    }

    #[test]
    fn dsh_settings_expose_per_model_reasoning_efforts() {
        let result = parse_dsh_settings_models(
            r#"
llm-pi-ai:
  providers:
    gpt:
      displayName: gpt
      models:
        - id: gpt-5.6-sol
          name: gpt-5.6-sol
          reasoningEfforts:
            off:
            low: low
            medium: medium
            high: high
            xhigh: xhigh
            max: max
        - id: gpt-image-2
          name: gpt-image-2
          reasoningEfforts: false
        - id: plain
          name: plain
"#,
        )
        .expect("parse dsh reasoning");
        let sol = result
            .reasoning_by_model
            .get("gpt:gpt-5.6-sol")
            .expect("sol efforts");
        assert_eq!(
            sol.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            vec!["default", "off", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            result
                .reasoning_by_model
                .get("gpt:gpt-image-2")
                .map(Vec::is_empty),
            Some(true)
        );
        assert_eq!(
            result
                .reasoning_by_model
                .get("gpt:plain")
                .map(Vec::is_empty),
            Some(true)
        );
        assert_eq!(
            result
                .reasoning_by_model
                .get("deepseek-v4-flash")
                .map(|items| items
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>()),
            Some(vec!["default", "off", "high", "max"])
        );
    }

    #[test]
    fn dsh_native_provider_summaries_expose_config_without_secrets() {
        let summaries = parse_dsh_native_provider_summaries(
            r#"
agent-default-model:
  provider: xiaobai
  model: gpt-test
llm-pi-ai:
  providers:
    xiaobai:
      displayName: XiaoBai
      apiKeyEnv: XIAOBAI_API_KEY
      api: openai-responses
      baseURL: https://relay.example/v1
      models:
        - id: gpt-test
          name: GPT Test
        - id: broken
"#,
        )
        .expect("parse dsh provider summaries");
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, "deepseek-official");
        assert_eq!(summaries[0].name, "DeepSeek");
        assert_eq!(summaries[0].model_count, 2);
        assert!(!summaries[0].is_default);
        assert_eq!(summaries[1].id, "xiaobai");
        assert_eq!(summaries[1].name, "XiaoBai");
        assert_eq!(
            summaries[1].base_url.as_deref(),
            Some("https://relay.example/v1")
        );
        assert_eq!(summaries[1].api.as_deref(), Some("openai-responses"));
        assert_eq!(summaries[1].model_count, 2);
        assert!(summaries[1].is_default);
        let json = serde_json::to_string(&summaries).unwrap();
        assert!(!json.contains("XIAOBAI_API_KEY"));
    }

    #[test]
    fn dsh_native_provider_summaries_always_include_official_deepseek() {
        let empty = parse_dsh_native_provider_summaries("{}").expect("empty dsh settings");
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].id, "deepseek-official");
        assert_eq!(empty[0].name, "DeepSeek");
        assert_eq!(empty[0].model_count, 2);
        assert!(empty[0].is_default);

        let official = parse_dsh_native_provider_summaries(
            r#"
agent-default-model:
  provider: deepseek-official
  model: deepseek-v4-flash
llm-deepseek:
  models:
    - id: deepseek-v4-flash
    - id: deepseek-v4-pro
    - id: ""
"#,
        )
        .expect("official dsh settings");
        assert_eq!(official.len(), 1);
        assert_eq!(official[0].model_count, 2);
        assert!(official[0].is_default);
    }
}
