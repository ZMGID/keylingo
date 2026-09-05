use super::{antigravity, header_pairs, http, now, resolve_provider, KIMI_BASE};
use crate::{settings::ModelProvider, state::AppState};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    label: String,
    used_percent: Option<f64>,
    used: Option<f64>,
    limit: Option<f64>,
    resets_at: Option<i64>,
    reset_hint: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    plan: Option<String>,
    windows: Vec<UsageWindow>,
    fetched_at: i64,
}
fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok()).filter(|n| n.is_finite() && *n >= 0.0)
}
fn reset(v: &Value, fetched: i64) -> Option<i64> {
    for key in ["reset_at", "resetAt", "reset_time", "resetTime"] {
        if let Some(n) = number(&v[key]) { return Some(if n > 1e12 { (n / 1000.0) as i64 } else { n as i64 }); }
        if let Some(t) = v[key].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) { return Some(t.timestamp()); }
    }
    ["reset_after_seconds", "reset_in", "resetIn", "ttl"].iter()
        .find_map(|key| number(&v[*key])).map(|n| fetched.saturating_add(n as i64))
}
fn kimi_window(v: &Value, label: String, fetched: i64) -> Option<UsageWindow> {
    let limit = number(&v["limit"]);
    let used = number(&v["used"]).or_else(|| Some((limit? - number(&v["remaining"])?).max(0.0)));
    if used.is_none() && limit.is_none() { return None; }
    Some(UsageWindow {
        label: v["name"].as_str().or(v["title"].as_str()).unwrap_or(&label).to_owned(),
        used_percent: used.zip(limit).filter(|(_, l)| *l > 0.0).map(|(u,l)| (100.0*u/l).clamp(0.0,100.0)),
        used, limit, resets_at: reset(v, fetched), reset_hint: None,
    })
}
fn parse_usage(kind: &str, value: &Value, fetched: i64) -> AccountUsage {
    let mut windows = Vec::new();
    if kind == "antigravity" {
        windows = antigravity_windows(value, fetched);
    } else if kind == "kimi" {
        if let Some(row) = kimi_window(&value["usage"], "Weekly".into(), fetched) { windows.push(row); }
        if let Some(limits) = value["limits"].as_array() {
            for (i, item) in limits.iter().enumerate() {
                let detail = item.get("detail").filter(|v| v.is_object()).unwrap_or(item);
                let duration = number(&item["window"]["duration"]);
                let unit = item["window"]["timeUnit"].as_str().unwrap_or("");
                let label = item["name"].as_str().or(item["title"].as_str()).map(str::to_owned).unwrap_or_else(|| match duration {
                    Some(n) if unit.contains("MINUTE") && n % 60.0 == 0.0 => format!("{}h", n/60.0),
                    Some(n) if unit.contains("MINUTE") => format!("{n}m"),
                    Some(n) if unit.contains("HOUR") => format!("{n}h"),
                    Some(n) if unit.contains("DAY") => format!("{n}d"),
                    _ => format!("Limit {}", i+1),
                });
                if let Some(mut row) = kimi_window(detail, label, fetched) {
                    row.resets_at = row.resets_at.or_else(|| reset(item, fetched));
                    windows.push(row);
                }
            }
        }
    } else {
        for (group, prefix) in [("rate_limit", ""), ("code_review_rate_limit", "Code review · ")] {
            for (key, fallback) in [("primary_window", "Session"), ("secondary_window", "Weekly")] {
                let v = &value[group][key];
                if !v.is_object() { continue; }
                let label = match number(&v["limit_window_seconds"]) {
                    Some(n) if n >= 3600.0 && n % 3600.0 == 0.0 => format!("{}h", n/3600.0),
                    _ => fallback.into(),
                };
                windows.push(UsageWindow { label: format!("{prefix}{label}"), used_percent: number(&v["used_percent"]).map(|n| n.clamp(0.0,100.0)), used: None, limit: None, resets_at: reset(v, fetched), reset_hint: None });
            }
        }
    }
    AccountUsage { plan: value["plan_type"].as_str().map(str::to_owned), windows, fetched_at: fetched }
}

fn antigravity_windows(value: &Value, fetched: i64) -> Vec<UsageWindow> {
    let value = value.get("response").filter(|v| v.is_object()).unwrap_or(value);
    let mut windows = Vec::new();
    let mut add = |bucket: &Value, group: &str, index: usize| {
        let remaining = bucket.get("remaining").filter(|v| v.is_object()).unwrap_or(bucket);
        let fraction = number(&remaining["remainingFraction"]).filter(|n| *n <= 1.0);
        let name = bucket["displayName"].as_str()
            .or(bucket["modelId"].as_str()).or(bucket["bucketId"].as_str()).or(bucket["window"].as_str())
            .map(str::to_owned).unwrap_or_else(|| format!("Quota {}", index + 1));
        let label = if group.is_empty() { name } else { format!("{group} · {name}") };
        windows.push(UsageWindow {
            label, used_percent: fraction.map(|n| (1.0-n)*100.0),
            used: None, limit: None,
            resets_at: reset(bucket, fetched).or_else(|| reset(remaining, fetched)),
            reset_hint: bucket["description"].as_str().map(str::to_owned),
        });
    };
    if let Some(groups) = value["groups"].as_array() {
        for group in groups {
            if let Some(buckets) = group["buckets"].as_array() {
                for (i, bucket) in buckets.iter().filter(|v| v.is_object()).enumerate() {
                    add(bucket, group["displayName"].as_str().unwrap_or(""), i);
                }
            }
        }
    }
    if let Some(buckets) = value["buckets"].as_array() {
        for (i, bucket) in buckets.iter().filter(|v| v.is_object()).enumerate() { add(bucket, "", i); }
    }
    windows
}

async fn antigravity_usage(provider: &ModelProvider) -> Result<AccountUsage, String> {
    let client = http(provider.request.use_system_proxy)?;
    let token = provider.preferred_api_key().ok_or("OAuth token missing")?;
    let project = provider.request.oauth.as_ref().and_then(|a| a.project_id.as_deref()).ok_or("Antigravity project missing")?;
    let body = serde_json::json!({"project": project});
    // Dedicated quota endpoints only: catalog availability can masquerade as 100% remaining.
    if let Ok(value) = antigravity::control(&client, token, "retrieveUserQuotaSummary", body.clone()).await {
        let result = parse_usage("antigravity", &value, now());
        if result.windows.iter().any(|w| w.used_percent.is_some()) { return Ok(result); }
    }
    let value = antigravity::control(&client, token, "retrieveUserQuota", body).await?;
    Ok(parse_usage("antigravity", &value, now()))
}

#[tauri::command]
pub async fn provider_oauth_usage(state: tauri::State<'_, AppState>, provider: ModelProvider) -> Result<AccountUsage, String> {
    let kind = provider.request.oauth.as_ref().ok_or("OAuth account required")?.provider.clone();
    if kind == "antigravity" {
        let resolved = resolve_provider(&state, &provider).await?;
        return antigravity_usage(&resolved).await;
    }
    let url = match kind.as_str() {
        "kimi" => format!("{KIMI_BASE}/usages"),
        "codex" => "https://chatgpt.com/backend-api/wham/usage".into(),
        _ => return Err("Usage is not available for this provider yet".into()),
    };
    let resolved = resolve_provider(&state, &provider).await?;
    let client = http(provider.request.use_system_proxy)?;
    let mut request = client.get(url).bearer_auth(resolved.preferred_api_key().ok_or("OAuth token missing")?).header("Accept", "application/json");
    for (key, value) in header_pairs(&resolved) { request = request.header(key, value); }
    let response = request.send().await.map_err(|_| "Usage request failed; check your connection or proxy")?;
    if !response.status().is_success() { return Err(format!("Usage request failed (HTTP {})", response.status().as_u16())); }
    let value: Value = response.json().await.map_err(|_| "Invalid usage response")?;
    Ok(parse_usage(&kind, &value, now()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn antigravity_grouped_and_model_quota() {
        let grouped = parse_usage("antigravity", &json!({"response":{"groups":[{"displayName":"Gemini","buckets":[{"displayName":"Weekly","remaining":{"remainingFraction":0.25},"resetTime":"2026-09-06T00:00:00Z"},{"displayName":"Session","remainingFraction":0}]}]}}), 100);
        assert_eq!(grouped.windows.len(), 2);
        assert_eq!(grouped.windows[0].label, "Gemini · Weekly");
        assert_eq!(grouped.windows[0].used_percent, Some(75.0));
        assert!(grouped.windows[0].resets_at.is_some());
        assert_eq!(grouped.windows[1].used_percent, Some(100.0));
        let model = parse_usage("antigravity", &json!({"buckets":[{"modelId":"gemini-3.8-flash","remainingFraction":1,"resetTime":"2026-09-06T00:00:00Z"}]}), 100);
        assert_eq!(model.windows[0].used_percent, Some(0.0));
    }
    #[test]
    fn antigravity_catalog_and_invalid_fractions_are_not_quota() {
        let catalog = json!({"models":{"gemini-3.8-flash":{"quotaInfo":{"remainingFraction":1}}}});
        assert!(parse_usage("antigravity", &catalog, 100).windows.is_empty());
        let result = parse_usage("antigravity", &json!({"buckets":[{"modelId":"missing"},{"modelId":"invalid","remainingFraction":2},{"modelId":"negative","remainingFraction":-1}]}), 100);
        assert_eq!(result.windows.len(), 3);
        assert!(result.windows.iter().all(|w| w.used_percent.is_none()));
    }
    #[test]
    fn kimi_remaining_and_windows() {
        let value = json!({"usage":{"limit":"100","remaining":"25","resetTime":"2026-09-06T00:00:00Z"},"limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},"detail":{"limit":20,"used":0,"resetIn":60}}]});
        let result = parse_usage("kimi", &value, 100);
        assert_eq!(result.windows[0].used_percent, Some(75.0));
        assert!(result.windows[0].resets_at.unwrap() > 100);
        assert_eq!(result.windows[1].label, "5h");
        assert_eq!(result.windows[1].used_percent, Some(0.0));
        assert_eq!(result.windows[1].resets_at, Some(160));
    }
    #[test]
    fn missing_usage_is_not_zero_or_full() {
        let result = parse_usage("kimi", &json!({"usage":{"limit":100}}), 100);
        assert_eq!(result.windows[0].used_percent, None);
        assert!(parse_usage("kimi", &json!({}), 100).windows.is_empty());
    }
    #[test]
    fn codex_percent_direction_and_reset() {
        let result = parse_usage("codex", &json!({"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":80,"reset_at":200,"limit_window_seconds":18000},"secondary_window":{"used_percent":0,"reset_after_seconds":30}}}), 100);
        assert_eq!(result.plan.as_deref(), Some("plus"));
        assert_eq!(result.windows[0].used_percent, Some(80.0));
        assert_eq!(result.windows[0].label, "5h");
        assert_eq!(result.windows[1].resets_at, Some(130));
    }
}
