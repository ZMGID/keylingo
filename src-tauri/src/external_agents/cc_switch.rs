//! 从 cc-switch 导入供应商。**只读**：`SQLITE_OPEN_READ_ONLY` 打开它的库，一个字节都不写。
//!
//! 数据形状（本机 v3 库实测，`~/.cc-switch/cc-switch.db` 的 `providers` 表）：
//! `id, app_type, name, settings_config(JSON), website_url, category, is_current, …`
//! `settings_config` 的形状按 `app_type` 各不相同：
//! - `claude` / `claude-desktop`：`{"env": {...}, 以及一堆 settings.json 偏好}`
//! - `codex`：`{"auth": {...}, "config": "<config.toml 全文>"}`
//! - `gemini`：`{"env": {...}, "config": {...}}`
//! - `grokbuild`：`{"config": "<config.toml 全文>"}`（落盘到 `~/.grok/config.toml`）
//! - `hermes` / `openclaw`：各自私有形状
//!
//! ponytail: 只导 Kivio **确实有落地通道**的四类（claude=env / codex=私有 CODEX_HOME /
//! gemini=env / grok=原生 config.toml）。hermes 等没有注入通道的，报成跳过数而不是假装支持。
//! 也没做 v2 `config.json` 回落：v3 的库存在就够了，缺了直接报「未找到」。
use crate::settings::CliEnvVar;

/// 一条可导入的供应商。**不含明文 key 的判断以外的任何加工**——`env` / `config_toml` 原样带过来，
/// 但前端列表只展示 `has_api_key`，不把 key 回显。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedProvider {
    /// 映射到 Kivio 的 agent id（claude / codex / gemini / grok）。
    pub agent_id: String,
    /// 保留 cc-switch 原 id：二次导入按 id 走更新，不会堆出重复条目。
    pub id: String,
    pub name: String,
    pub remark: String,
    pub env: Vec<CliEnvVar>,
    pub config_toml: String,
    pub auth_json: String,
    pub has_api_key: bool,
    /// 它在 cc-switch 里是当前生效的那条。
    pub is_current: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportScan {
    pub providers: Vec<ImportedProvider>,
    /// 认得出但 Kivio 没有落地通道而跳过的条数（hermes / openclaw…）。
    pub skipped: usize,
}

fn db_path() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|base| base.home_dir().join(".cc-switch/cc-switch.db"))
}

/// cc-switch 的 `app_type` → Kivio 的 agent id。返回 None = 没有落地通道，跳过。
fn map_app_type(app_type: &str) -> Option<&'static str> {
    match app_type {
        "claude" | "claude-code" | "claude_code" => Some("claude"),
        "codex" => Some("codex"),
        "gemini" => Some("gemini"),
        "grokbuild" | "grok" | "grok-cli" => Some("grok"),
        _ => None,
    }
}

pub fn scan() -> Result<ImportScan, String> {
    let path = db_path().ok_or_else(|| "找不到用户主目录".to_string())?;
    if !path.is_file() {
        return Err(format!("未找到 cc-switch 数据：{}", path.display()));
    }
    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开 cc-switch 数据库失败：{e}"))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, app_type, name, settings_config, website_url, is_current \
             FROM providers ORDER BY app_type, COALESCE(sort_index, 999999), COALESCE(created_at, 0)",
        )
        .map_err(|e| format!("读取 cc-switch 供应商失败：{e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
            ))
        })
        .map_err(|e| format!("读取 cc-switch 供应商失败：{e}"))?;

    let mut providers = Vec::new();
    let mut skipped = 0usize;
    for row in rows {
        let Ok((id, app_type, name, settings_config, website, is_current)) = row else {
            skipped += 1;
            continue;
        };
        let Some(agent_id) = map_app_type(&app_type) else {
            skipped += 1;
            continue;
        };
        // 单条 JSON 解析失败不拖垮整批（cc-switch 的库里混过手工编辑的条目）。
        let Ok(config) = serde_json::from_str::<serde_json::Value>(&settings_config) else {
            skipped += 1;
            continue;
        };
        match convert(
            agent_id,
            id,
            name,
            website.unwrap_or_default(),
            is_current,
            &config,
        ) {
            Some(provider) => providers.push(provider),
            None => skipped += 1,
        }
    }
    Ok(ImportScan { providers, skipped })
}

fn convert(
    agent_id: &str,
    id: String,
    name: String,
    remark: String,
    is_current: bool,
    config: &serde_json::Value,
) -> Option<ImportedProvider> {
    let mut out = ImportedProvider {
        agent_id: agent_id.to_string(),
        id,
        name,
        remark,
        env: Vec::new(),
        config_toml: String::new(),
        auth_json: String::new(),
        has_api_key: false,
        is_current,
    };
    if agent_id == "codex" {
        out.config_toml = config.get("config")?.as_str()?.to_string();
        if let Some(auth) = config.get("auth").filter(|v| v.is_object()) {
            out.has_api_key = auth
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .is_some_and(|key| !key.trim().is_empty());
            out.auth_json = serde_json::to_string_pretty(auth).ok()?;
        }
        return Some(out);
    }
    if agent_id == "grok" {
        // cc-switch grokbuild：`{"config": "<config.toml 全文>"}`。空壳或只有空白的跳过。
        let toml = config.get("config")?.as_str()?.to_string();
        if toml.trim().is_empty() {
            return None;
        }
        // 粗嗅：至少要有 model 路由，否则导进来也落不了盘。
        if !toml.contains("[model") && !toml.contains("base_url") {
            return None;
        }
        out.config_toml = toml;
        out.has_api_key = out.config_toml.lines().any(|line| {
            line.trim_start().starts_with("api_key") && line.contains('=') && !line.contains("\"\"")
        });
        return Some(out);
    }
    // claude / gemini：只取 `env`。cc-switch 的 claude 条目里还带着 theme / permissions /
    // enabledPlugins 这些 settings.json 偏好——那是**用户全局配置**，不是供应商路由，
    // 带进来会通过 `--settings` 覆盖掉用户自己的插件和权限设置。
    let env = config.get("env")?.as_object()?;
    out.env = env
        .iter()
        .filter_map(|(key, value)| {
            Some(CliEnvVar {
                key: key.clone(),
                value: value.as_str()?.to_string(),
            })
        })
        .collect();
    if out.env.is_empty() {
        return None;
    }
    out.has_api_key = out.env.iter().any(|pair| {
        matches!(
            pair.key.as_str(),
            "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY" | "GEMINI_API_KEY" | "GOOGLE_API_KEY"
        ) && !pair.value.trim().is_empty()
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_takes_env_only() {
        let config = json!({
            "env": { "ANTHROPIC_BASE_URL": "https://relay.example/anthropic", "ANTHROPIC_AUTH_TOKEN": "sk-x" },
            "permissions": { "allow": ["Bash"] },
            "enabledPlugins": ["a@b"],
        });
        let p = convert(
            "claude",
            "p1".into(),
            "Relay".into(),
            String::new(),
            true,
            &config,
        )
        .unwrap();
        assert_eq!(p.env.len(), 2);
        assert!(p.has_api_key);
        assert!(p.config_toml.is_empty());
    }

    #[test]
    fn codex_takes_toml_and_auth() {
        let config = json!({
            "auth": { "OPENAI_API_KEY": "sk-y" },
            "config": "model = \"gpt-5.6\"\nmodel_provider = \"custom\"\n",
        });
        let p = convert(
            "codex",
            "c1".into(),
            "Codex Relay".into(),
            String::new(),
            false,
            &config,
        )
        .unwrap();
        assert!(p.config_toml.contains("model_provider"));
        assert!(p.auth_json.contains("OPENAI_API_KEY"));
        assert!(p.has_api_key);
    }

    #[test]
    fn env_less_entry_is_skipped() {
        // gemini 条目常常是空壳 `{"env":{},"config":{}}`，导进来是个不能用的空供应商。
        assert!(convert(
            "gemini",
            "g1".into(),
            "G".into(),
            String::new(),
            false,
            &json!({"env":{},"config":{}})
        )
        .is_none());
    }

    #[test]
    fn unsupported_app_types_map_to_none() {
        assert!(map_app_type("hermes").is_none());
        assert_eq!(map_app_type("claude"), Some("claude"));
        assert_eq!(map_app_type("grokbuild"), Some("grok"));
        assert_eq!(map_app_type("grok"), Some("grok"));
    }

    #[test]
    fn grok_takes_config_toml() {
        let config = json!({
            "config": "[models]\ndefault = \"grok-4.5\"\n\n[model.\"grok-4.5\"]\nmodel = \"grok-4.5\"\nbase_url = \"https://relay.example/v1\"\napi_key = \"sk-x\"\n"
        });
        let p = convert(
            "grok",
            "g1".into(),
            "Relay".into(),
            String::new(),
            true,
            &config,
        )
        .unwrap();
        assert!(p.config_toml.contains("base_url"));
        assert!(p.has_api_key);
        assert!(p.env.is_empty());
    }

    #[test]
    fn grok_empty_config_is_skipped() {
        assert!(convert(
            "grok",
            "g1".into(),
            "Empty".into(),
            String::new(),
            false,
            &json!({"config": ""})
        )
        .is_none());
        assert!(convert(
            "grok",
            "g2".into(),
            "Official".into(),
            String::new(),
            false,
            &json!({"config": ""})
        )
        .is_none());
    }
}
