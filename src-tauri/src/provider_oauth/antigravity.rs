//! Protocol adapted from Rahul Arya's pi-antigravity (MIT); see docs/licenses/.
use super::*;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub const BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
pub(super) const CLIENT: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
// Public installed desktop client value, not an account credential.
pub(super) const CLIENT_SECRET: &str = "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";
pub(super) const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REDIRECT: &str = "http://localhost:51121/oauth-callback";
pub fn user_agent() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("antigravity/cli/1.1.23 (aidev_client; os_type={}; arch={arch}; cl=974125021; auth_method=consumer)", std::env::consts::OS)
}

struct Session {
    task: tokio::task::JoinHandle<Result<Tokens, String>>,
    expires: i64,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(Default::default)
}
fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn is_provider(p: &ModelProvider) -> bool {
    p.request
        .oauth
        .as_ref()
        .is_some_and(|a| a.provider == "antigravity")
}

pub(crate) fn model_includes_effort(model: &str) -> bool {
    model.trim().rsplit_once('-').is_some_and(|(_, suffix)| {
        matches!(suffix.to_ascii_lowercase().as_str(), "low" | "medium" | "high")
    })
}
pub fn unwrap_response(mut value: Value) -> Value {
    if value.get("response").is_some() && value.get("error").is_none() {
        value["response"].take()
    } else {
        value
    }
}

pub(super) async fn start(proxy: bool) -> Result<Login, String> {
    let mut active = sessions().lock().await;
    active.retain(|_, s| s.expires > now());
    if !active.is_empty() {
        return Err("Antigravity login is already pending; cancel it first".into());
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 51121))
        .await
        .map_err(|_| {
            "Cannot listen on localhost:51121; close another Antigravity login and retry"
        })?;
    let verifier = random_secret();
    let state = random_secret();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("client_id", CLIENT), ("response_type", "code"), ("redirect_uri", REDIRECT),
        ("scope", "https://www.googleapis.com/auth/aicode https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs"),
        ("code_challenge", &challenge), ("code_challenge_method", "S256"),
        ("state", &state), ("access_type", "offline"), ("prompt", "consent"),
    ]);
    let id = uuid::Uuid::new_v4().to_string();
    let expires = now() + 300;
    let task = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(300),
            authorize(listener, state, verifier, proxy),
        )
        .await
        .map_err(|_| "Antigravity login expired; start again".to_string())?
    });
    active.insert(id.clone(), Session { task, expires });
    Ok(Login {
        login_id: id,
        user_code: String::new(),
        verification_url: url.into(),
        interval: 3,
        expires_at: expires,
    })
}

// Reject unexpected paths, methods, duplicate parameters and state before accepting a code.
fn callback(request: &str, state: &str) -> Result<Option<String>, &'static str> {
    let mut line = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    if line.next() != Some("GET") {
        return Err("Invalid callback method");
    }
    let target = line.next().ok_or("Missing callback path")?;
    if !target.starts_with("/oauth-callback?") {
        return Err("Invalid callback path");
    }
    let url = url::Url::parse(&format!("http://localhost:51121{target}"))
        .map_err(|_| "Invalid callback URL")?;
    if url.path() != "/oauth-callback" {
        return Err("Invalid callback path");
    }
    let params: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = params.iter().filter(|(k, _)| k == "state").collect();
    if states.len() != 1 || states[0].1 != state {
        return Err("OAuth state mismatch");
    }
    if params.iter().any(|(k, _)| k == "error") {
        return Ok(None);
    }
    let codes: Vec<_> = params.iter().filter(|(k, _)| k == "code").collect();
    if codes.len() != 1 || codes[0].1.is_empty() {
        return Err("Missing authorization code");
    }
    Ok(Some(codes[0].1.to_string()))
}

async fn authorize(
    listener: TcpListener,
    state: String,
    verifier: String,
    proxy: bool,
) -> Result<Tokens, String> {
    let code = loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|_| "OAuth callback listener failed")?;
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            loop {
                let mut block = [0; 1024];
                let n = socket
                    .read(&mut block)
                    .await
                    .map_err(|_| "Callback read failed")?;
                if n == 0 {
                    return Err("Incomplete callback");
                }
                bytes.extend_from_slice(&block[..n]);
                if bytes.len() > 8192 {
                    return Err("Callback too large");
                }
                if bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                    break;
                }
            }
            callback(
                std::str::from_utf8(&bytes).map_err(|_| "Invalid callback encoding")?,
                &state,
            )
        })
        .await;
        let accepted = matches!(result, Ok(Ok(_)));
        let status = if accepted {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let body = if accepted {
            "Authorization received. Return to Kivio to finish signing in."
        } else {
            "Invalid OAuth callback. Return to your Google sign-in page."
        };
        let reply = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let _ =
            tokio::time::timeout(Duration::from_secs(2), socket.write_all(reply.as_bytes())).await;
        if let Ok(Ok(code)) = result {
            break code.ok_or("Google authorization was declined")?;
        }
    };
    drop(listener);
    let client = http(proxy)?;
    let (status, value) = response_json(client.post(TOKEN_URL).form(&[
        ("client_id", CLIENT),
        ("client_secret", CLIENT_SECRET),
        ("code", &code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", REDIRECT),
        ("code_verifier", &verifier),
    ]))
    .await?;
    if status != 200 {
        return Err(format!("Antigravity token exchange failed (HTTP {status})"));
    }
    let mut tokens = tokens_from(&value, "antigravity", "")?;
    let value = control(
        &client,
        &tokens.access_token,
        "loadCodeAssist",
        json!({"metadata":{"ideType":"ANTIGRAVITY"}}),
    )
    .await?;
    tokens.project_id = project_id(&value);
    if tokens.project_id.is_none() {
        return Err("No Antigravity project is available for this account. Complete account setup in Antigravity, then sign in again.".into());
    }
    Ok(tokens)
}

fn project_id(value: &Value) -> Option<String> {
    [
        "cloudaicompanionProject",
        "projectId",
        "antigravityProjectId",
        "project",
    ]
    .iter()
    .find_map(|k| {
        value[*k]
            .as_str()
            .or_else(|| value[*k]["id"].as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    })
}

pub(super) async fn poll(id: &str) -> Option<Result<PollResult, String>> {
    let mut active = sessions().lock().await;
    let session = active.get_mut(id)?;
    if !session.task.is_finished() && session.expires > now() {
        return Some(Ok(PollResult {
            status: "pending",
            interval: 3,
            auth: None,
        }));
    }
    let result = if session.expires <= now() {
        Err("Antigravity login expired; start again".into())
    } else {
        match (&mut session.task).await {
            Ok(Ok(tokens)) => save_tokens(id, &tokens).map(|_| PollResult {
                status: "authorized",
                interval: 0,
                auth: Some(OAuthConfig {
                    provider: "antigravity".into(),
                    credential_id: Some(id.into()),
                    project_id: None,
                }),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("Antigravity login was interrupted".into()),
        }
    };
    active.remove(id);
    Some(result)
}

pub(super) async fn cancel(id: &str) {
    if let Some(mut session) = sessions().lock().await.remove(id) {
        session.task.abort();
        let _ = (&mut session.task).await;
    }
}

pub(super) async fn control(
    client: &reqwest::Client,
    token: &str,
    method: &str,
    body: Value,
) -> Result<Value, String> {
    let (status, value) = response_json(
        client
            .post(format!("{BASE}/v1internal:{method}"))
            .bearer_auth(token)
            .header("User-Agent", user_agent())
            .json(&body),
    )
    .await?;
    if status != 200 {
        return Err(format!(
            "Antigravity {method} failed (HTTP {status}); check account access and quota"
        ));
    }
    Ok(value)
}

pub(super) async fn models(provider: &ModelProvider) -> Result<Vec<String>, String> {
    let value = control(
        &http(provider.request.use_system_proxy)?,
        provider
            .preferred_api_key()
            .ok_or("Missing OAuth session")?,
        "fetchAvailableModels",
        json!({"project": provider.request.oauth.as_ref().and_then(|a| a.project_id.as_deref())}),
    )
    .await?;
    parse_models(&value)
}
fn parse_models(value: &Value) -> Result<Vec<String>, String> {
    let map = value["models"]
        .as_object()
        .ok_or("Invalid Antigravity model catalog")?;
    let mut ids: Vec<String> = map
        .iter()
        .filter(|(id, v)| {
            ["gemini-", "claude-", "gpt-oss-"]
                .iter()
                .any(|p| id.starts_with(p))
                && !id.chars().any(char::is_whitespace)
                && v["visibility"] != "hide"
        })
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

pub fn wrap_request(provider: &ModelProvider, model: &str, mut body: Value) -> Value {
    let custom = model.starts_with("claude-") || model.starts_with("gpt-oss-");
    if let Some(tools) = body["tools"].as_array_mut() {
        tools.retain(|t| t["functionDeclarations"].is_array());
        for tool in tools {
            if let Some(decls) = tool["functionDeclarations"].as_array_mut() {
                for decl in decls {
                    if let Some(schema) = decl.as_object_mut().and_then(|d| d.remove("parameters"))
                    {
                        decl[if custom {
                            "parameters"
                        } else {
                            "parametersJsonSchema"
                        }] = if custom {
                            custom_schema(&schema)
                        } else {
                            schema
                        };
                    }
                }
            }
        }
    }
    if body.get("toolConfig").is_some() || custom {
        body["toolConfig"] = json!({"functionCallingConfig":{"mode":"VALIDATED"}});
    }
    // Runtime catalog IDs already encode reasoning variants. Do not send Gemini's
    // thinkingLevel dialect to the Claude/GPT-OSS bridge.
    if custom {
        if let Some(config) = body["generationConfig"].as_object_mut() {
            config.remove("thinkingConfig");
        }
    } else if model_includes_effort(model) {
        // The catalog model owns its effort. Keep thought output, but discard
        // independent overrides from old conversations or auxiliary requests.
        if let Some(thinking) = body["generationConfig"]["thinkingConfig"].as_object_mut() {
            thinking.remove("thinkingLevel");
            thinking.remove("thinkingBudget");
        }
    }
    if let Some(level) = body["generationConfig"]["thinkingConfig"]["thinkingLevel"].as_str() {
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"] =
            json!(level.to_ascii_uppercase());
    }
    if body.get("systemInstruction").is_some() {
        body["systemInstruction"]["role"] = json!("user");
    }
    if let Some(contents) = body["contents"].as_array_mut() {
        if contents.first().is_some_and(|v| v["role"] == "model") {
            contents.insert(0, json!({"role":"user","parts":[{"text":"Continue."}]}));
        }
    }
    body["sessionId"] = json!(rand::random::<i64>().to_string());
    json!({"project": provider.request.oauth.as_ref().and_then(|a| a.project_id.as_deref()),
        "model": model, "request": body, "requestType":"agent", "userAgent":"antigravity",
        "requestId": format!("agent/{}", uuid::Uuid::new_v4())})
}
fn custom_schema(value: &Value) -> Value {
    let Some(map) = value.as_object() else {
        return value.clone();
    };
    let mut out = serde_json::Map::new();
    for (k, v) in map {
        if k == "enum"
            && v.as_array()
                .is_some_and(|a| a.iter().any(|v| !v.is_string()))
        {
            continue;
        }
        if ![
            "type",
            "description",
            "properties",
            "required",
            "items",
            "enum",
        ]
        .contains(&k.as_str())
        {
            continue;
        }
        let next = if k == "properties" {
            Value::Object(
                v.as_object()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| (k.clone(), custom_schema(v)))
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        } else if k == "type" && v.is_array() {
            v.as_array()
                .and_then(|a| a.iter().find(|v| v.is_string() && *v != "null"))
                .cloned()
                .unwrap_or(json!("string"))
        } else {
            custom_schema(v)
        };
        out.insert(k.clone(), next);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancel_releases_listener_without_saving_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let task = tokio::spawn(authorize(
            listener,
            "test-state".into(),
            "test-verifier".into(),
            false,
        ));
        sessions().lock().await.insert(
            id.clone(),
            Session {
                task,
                expires: now() + 300,
            },
        );
        assert_eq!(poll(&id).await.unwrap().unwrap().status, "pending");
        cancel(&id).await;
        let _listener = TcpListener::bind(address).await.unwrap();
        assert!(poll(&id).await.is_none());
    }
    #[tokio::test]
    async fn callback_rejects_bad_state_then_accepts_denial() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(authorize(
            listener,
            "expected".into(),
            "verifier".into(),
            false,
        ));
        for (state, status) in [("wrong", "400 Bad Request"), ("expected", "200 OK")] {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            socket.write_all(format!("GET /oauth-callback?state={state}&error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes()).await.unwrap();
            let mut response = String::new();
            tokio::time::timeout(Duration::from_secs(5), socket.read_to_string(&mut response))
                .await
                .unwrap()
                .unwrap();
            assert!(response.contains(status));
            assert!(!response.contains(state));
        }
        assert!(task.await.unwrap().err().unwrap().contains("declined"));
        let _listener = TcpListener::bind(address).await.unwrap();
    }
    #[test]
    fn validates_callback() {
        assert_eq!(
            callback(
                "GET /oauth-callback?state=ok&code=a%2Bb HTTP/1.1\r\n\r\n",
                "ok"
            ),
            Ok(Some("a+b".into()))
        );
        for path in [
            "/other?state=ok&code=a",
            "/oauth-callback?state=bad&code=a",
            "/oauth-callback?state=ok&state=ok&code=a",
            "/oauth-callback?state=ok&code=a&code=b",
        ] {
            assert!(callback(&format!("GET {path} HTTP/1.1"), "ok").is_err());
        }
        assert_eq!(
            callback(
                "GET /oauth-callback?state=ok&error=access_denied HTTP/1.1",
                "ok"
            ),
            Ok(None)
        );
    }
    #[test]
    fn catalog_uses_runtime_keys() {
        assert_eq!(parse_models(&json!({"models":{"claude-example":{"model":"MODEL_PLACEHOLDER_1"},"MODEL_PLACEHOLDER_2":{},"gemini-hidden":{"visibility":"hide"}}})).unwrap(), vec!["claude-example"]);
        assert!(project_id(&json!({})).is_none());
        assert_eq!(
            project_id(&json!({"cloudaicompanionProject":{"id":"owned"}})),
            Some("owned".into())
        );
    }
    #[test]
    fn schema_keeps_property_names() {
        let schema = custom_schema(
            &json!({"type":"object","additionalProperties":false,"properties":{"default":{"type":["string","null"],"default":"x"}},"required":["default"]}),
        );
        assert_eq!(schema["properties"]["default"]["type"], "string");
        assert!(schema["properties"]["default"].get("default").is_none());
        assert_eq!(schema["required"], json!(["default"]));
    }
}
