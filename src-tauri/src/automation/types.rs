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
    "logic.delay",
    "agent.runtime",
    "agent.context",
    "agent.tool",
    "agent.skill",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
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
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    #[serde(default)]
    pub edges: Vec<FlowEdge>,
    #[serde(default)]
    pub viewport: Viewport,
    pub created_at: String,
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
}

impl RunOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Hotkey => "hotkey",
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
}
