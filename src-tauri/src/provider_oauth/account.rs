use super::{claims, http, read_tokens, refresh_lock, resolve_provider};
use crate::{settings::ModelProvider, state::AppState};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    email: Option<String>,
    name: Option<String>,
    account_id: Option<String>,
}
fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}
// Display metadata only. JWT decoding is not used as an authorization decision.
fn identity(kind: &str, access: &Value, refresh: &Value) -> AccountIdentity {
    let profile = &access["https://api.openai.com/profile"];
    let auth = &access["https://api.openai.com/auth"];
    let account_id = if kind == "codex" {
        text(&auth["chatgpt_account_id"]).or_else(|| text(&auth["chatgpt_user_id"]))
            .or_else(|| text(&access["sub"]))
    } else {
        text(&access["user_id"]).or_else(|| text(&refresh["user_id"]))
            .or_else(|| text(&access["sub"])).or_else(|| text(&refresh["sub"]))
    };
    AccountIdentity {
        email: text(&access["email"]).or_else(|| text(&profile["email"])).or_else(|| text(&refresh["email"])),
        name: text(&access["name"]).or_else(|| text(&profile["name"])).or_else(|| text(&access["username"])),
        account_id,
    }
}
#[tauri::command]
pub async fn provider_oauth_account(state: tauri::State<'_, AppState>, provider: ModelProvider) -> Result<AccountIdentity, String> {
    let resolved = resolve_provider(&state, &provider).await?;
    let auth = resolved.request.oauth.as_ref().ok_or("OAuth account required")?;
    let id = auth.credential_id.as_deref().ok_or("Sign in first")?;
    if auth.provider == "antigravity" {
        let response = http(provider.request.use_system_proxy)?
            .get("https://openidconnect.googleapis.com/v1/userinfo")
            .bearer_auth(resolved.preferred_api_key().ok_or("OAuth token missing")?)
            .send().await.map_err(|_| "Could not fetch account profile")?;
        if !response.status().is_success() {
            return Err(format!("Account profile request failed (HTTP {})", response.status().as_u16()));
        }
        let value: Value = response.json().await.map_err(|_| "Invalid account profile")?;
        return Ok(identity("antigravity", &value, &Value::Null));
    }
    let _guard = refresh_lock().lock().await;
    let tokens = read_tokens(id)?;
    if tokens.provider != auth.provider { return Err("OAuth credential belongs to another provider".into()); }
    Ok(identity(&auth.provider, &claims(&tokens.access_token), &claims(&tokens.refresh_token)))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn provider_identity_fields_are_whitelisted() {
        let kimi = identity("kimi", &json!({"sub":"weaker","client_id":"not-account","access_token":"secret"}), &json!({"user_id":"kimi-user"}));
        assert_eq!(kimi.account_id.as_deref(), Some("kimi-user"));
        assert!(kimi.email.is_none());
        assert!(!serde_json::to_string(&kimi).unwrap().contains("secret"));
        let codex = identity("codex", &json!({"https://api.openai.com/profile":{"email":"person@example.com"},"https://api.openai.com/auth":{"chatgpt_account_id":"workspace"}}), &Value::Null);
        assert_eq!(codex.email.as_deref(), Some("person@example.com"));
        assert_eq!(codex.account_id.as_deref(), Some("workspace"));
        let google = identity("antigravity", &json!({"email":"google@example.com","name":"Person","sub":"google-user"}), &Value::Null);
        assert_eq!(google.name.as_deref(), Some("Person"));
        assert_eq!(google.email.as_deref(), Some("google@example.com"));
        assert!(identity("kimi", &Value::Null, &Value::Null).account_id.is_none());
    }
}
