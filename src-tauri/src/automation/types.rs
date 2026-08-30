use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

pub const ALLOWED_NODE_TYPES: &[&str] = &[
    "trigger.manual",
    "trigger.schedule",
    "trigger.hotkey",
    "action.agent",
    "action.notify",
    "action.http",
    "action.set",
    "action.clipboard",
    "action.file",
    "action.command",
    "logic.if",
    "logic.switch",
    "logic.delay",
    "agent.runtime",
    "agent.context",
    "agent.tool",
    "agent.skill",
];

pub fn allowed_node_types_csv() -> String {
    ALLOWED_NODE_TYPES.join(", ")
}

/// Compact schema returned on `automation_upsert` validation errors so the
/// model can fix a graph without probing. The always-on tool description stays
/// short; the `automation` skill is the authoring guide.
pub const UPSERT_MINIMAL_EXAMPLE: &str = r#"{
  "name": "AI briefing",
  "enabled": false,
  "nodes": [
    {"id": "t", "type": "trigger.manual", "data": {"label": "Manual"}},
    {"id": "a", "type": "action.agent", "data": {"label": "Write"}},
    {"id": "ctx", "type": "agent.context", "data": {"label": "Prompt", "agent": {"prompt": "Do the user's task using {{output}}."}}},
    {"id": "rt", "type": "agent.runtime", "data": {"label": "Runtime", "agent": {"runtimeKind": "builtin"}}},
    {"id": "n", "type": "action.notify", "data": {"label": "Notify", "notify": {"body": "{{output}}"}}}
  ],
  "edges": [
    {"id": "e1", "source": "t", "target": "a"},
    {"id": "e2", "source": "a", "target": "n"},
    {"id": "er", "source": "rt", "target": "a", "sourceHandle": "slot", "targetHandle": "runtime"},
    {"id": "ec", "source": "ctx", "target": "a", "sourceHandle": "slot", "targetHandle": "context"}
  ]
}"#;

pub fn upsert_schema_hint() -> String {
    format!(
        "Allowed node types (exact strings): {types}.\n\
Submit ONE complete graph. Do NOT glob the repo and do NOT dry_run-probe types one at a time. dry_run is only for a finished candidate.\n\
Node data:\n\
- trigger.manual: {{}}\n\
- trigger.schedule: data.schedule {{kind: daily|weekdays|interval, hour 0-23, minute 0-59, intervalMinutes}}\n\
- trigger.hotkey: data.hotkey {{accelerator e.g. CommandOrControl+Shift+B}}\n\
- action.agent: label only. Put prompt/runtime/tools/skills on slot nodes (agent.context / agent.runtime / agent.tool / agent.skill) with edges sourceHandle=slot, targetHandle=runtime|context|tool|skill, target=the action.agent id.\n\
- action.notify: data.notify {{body}}  action.http: data.http {{method GET|POST|PUT|PATCH|DELETE, url, headers, body}}\n\
- action.set: data.set {{fields:[{{key,value}}]}}  action.clipboard: data.clipboard {{op: copy|read, text}}\n\
- action.file: data.file {{op: read|write, path, content}}  action.command: data.command {{command, cwd, timeoutSeconds, continueOnFail}}\n\
- logic.if: data.if {{op: contains|equals|notEmpty, value}}; outgoing sourceHandle true|false\n\
- logic.switch: data.switch {{cases:[{{id, op, value}}]}}; handles = case ids + default\n\
- logic.delay: data.delay {{seconds>=1}}\n\
- agent.runtime: data.agent {{runtimeKind: builtin|chat|external, providerId, model, externalAgentId}}\n\
- agent.context: data.agent {{prompt}}  agent.tool: data.agent {{toolIds}}  agent.skill: data.agent {{skillIds}}\n\
Main edges: {{id, source, target}}. Templates: {{{{output}}}} previous text; {{{{json.path}}}} previous JSON; automation_run input is {{{{json.input.*}}}}.\n\
Omit node positions (auto-layout). Leave enabled=false until the user wants schedule/hotkey live. PDF/Word/Excel output = action.agent + agent.skill skillIds pdf|docx|xlsx, not a dedicated node.\n\
Minimal example: {example}",
        types = allowed_node_types_csv(),
        example = UPSERT_MINIMAL_EXAMPLE,
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Default for Vec2 {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub position: Vec2,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    #[serde(default)]
    pub edges: Vec<FlowEdge>,
    #[serde(default)]
    pub viewport: Viewport,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationMeta {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger_type: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunOrigin {
    Manual,
    Schedule,
    Hotkey,
    Agent,
}

impl RunOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Hotkey => "hotkey",
            Self::Agent => "agent",
        }
    }

    pub fn is_production(self) -> bool {
        matches!(self, Self::Schedule | Self::Hotkey)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeOutput {
    pub text: String,
    pub json: serde_json::Value,
}

impl NodeOutput {
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            json: serde_json::json!({ "text": text }),
            text,
        }
    }

    pub fn with_json(text: impl Into<String>, json: serde_json::Value) -> Self {
        Self {
            text: text.into(),
            json,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRunEvent {
    pub kind: String,
    pub automation_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRunNode {
    pub node_id: String,
    pub node_type: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRun {
    pub id: String,
    pub automation_id: String,
    pub origin: String,
    pub status: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub nodes: Vec<AutomationRunNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRunSummary {
    pub id: String,
    pub origin: String,
    pub status: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRunStarted {
    pub run_id: String,
}

impl Automation {
    pub fn meta(&self) -> AutomationMeta {
        let trigger_type = self
            .nodes
            .iter()
            .find(|node| node.node_type.starts_with("trigger."))
            .map(|node| node.node_type.clone());
        AutomationMeta {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: self.enabled,
            trigger_type,
            updated_at: self.updated_at.clone(),
        }
    }
}

pub fn is_allowed_node_type(node_type: &str) -> bool {
    ALLOWED_NODE_TYPES.contains(&node_type)
}

pub fn is_slot_target_handle(handle: Option<&str>) -> bool {
    matches!(handle, Some("runtime" | "context" | "tool" | "skill"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationChangedEvent {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub severity: String,
    pub message: String,
}

impl ValidationIssue {
    pub fn error(node_id: Option<String>, message: impl Into<String>) -> Self {
        Self {
            node_id,
            severity: "error".into(),
            message: message.into(),
        }
    }

    pub fn warning(node_id: Option<String>, message: impl Into<String>) -> Self {
        Self {
            node_id,
            severity: "warning".into(),
            message: message.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == "error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_picks_first_trigger() {
        let automation = Automation {
            schema_version: SCHEMA_VERSION,
            id: "a1".into(),
            name: "demo".into(),
            enabled: false,
            nodes: vec![FlowNode {
                id: "n1".into(),
                node_type: "trigger.hotkey".into(),
                position: Vec2 { x: 0.0, y: 0.0 },
                data: serde_json::json!({}),
            }],
            edges: vec![],
            viewport: Viewport::default(),
            created_at: "t0".into(),
            updated_at: "t1".into(),
        };
        assert_eq!(automation.meta().trigger_type.as_deref(), Some("trigger.hotkey"));
    }

    #[test]
    fn upsert_example_deserializes_without_ids_or_timestamps() {
        let automation: Automation = serde_json::from_str(UPSERT_MINIMAL_EXAMPLE).expect("example json");
        assert_eq!(automation.name, "AI briefing");
        assert_eq!(automation.nodes.len(), 5);
        assert!(automation.id.is_empty());
        assert_eq!(automation.schema_version, 0);
    }
}
