//! Graph lint for agent-authored automations. Canvas saves skip this and stay
//! what-you-see-is-what-you-get; `automation_upsert` always runs it first.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use super::types::{
    is_allowed_node_type, is_slot_target_handle, Automation, FlowEdge, FlowNode, ValidationIssue,
    Vec2,
};

const LAYOUT_DX: f64 = 280.0;
const LAYOUT_DY: f64 = 140.0;
const SLOT_DY: f64 = 110.0;

const IF_OPS: &[&str] = &["contains", "equals", "notEmpty"];
const SCHEDULE_KINDS: &[&str] = &["daily", "weekdays", "interval"];
const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];
const CLIPBOARD_OPS: &[&str] = &["copy", "read"];
const FILE_OPS: &[&str] = &["read", "write"];

pub fn validate(automation: &Automation) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if automation.nodes.is_empty() {
        issues.push(ValidationIssue::error(None, "automation has no nodes"));
        return issues;
    }

    let mut ids = HashSet::new();
    for node in &automation.nodes {
        if node.id.trim().is_empty() {
            issues.push(ValidationIssue::error(
                None,
                "a node is missing an id",
            ));
            continue;
        }
        if !ids.insert(node.id.as_str()) {
            issues.push(ValidationIssue::error(
                Some(node.id.clone()),
                format!("duplicate node id '{}'", node.id),
            ));
        }
        if !is_allowed_node_type(&node.node_type) {
            issues.push(ValidationIssue::error(
                Some(node.id.clone()),
                format!(
                    "unknown node type '{}'; allowed: {}",
                    node.node_type,
                    super::types::allowed_node_types_csv()
                ),
            ));
        }
    }

    let node_by_id: HashMap<&str, &FlowNode> = automation
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    let trigger_count = automation
        .nodes
        .iter()
        .filter(|node| node.node_type.starts_with("trigger."))
        .count();
    if trigger_count == 0 {
        issues.push(ValidationIssue::error(
            None,
            "automation needs at least one trigger (trigger.manual, trigger.schedule, or trigger.hotkey)",
        ));
    }

    for edge in &automation.edges {
        if !node_by_id.contains_key(edge.source.as_str()) {
            issues.push(ValidationIssue::error(
                Some(edge.source.clone()),
                format!("edge '{}' source '{}' does not exist", edge.id, edge.source),
            ));
        }
        if !node_by_id.contains_key(edge.target.as_str()) {
            issues.push(ValidationIssue::error(
                Some(edge.target.clone()),
                format!("edge '{}' target '{}' does not exist", edge.id, edge.target),
            ));
        }
        if is_slot_target_handle(edge.target_handle.as_deref()) {
            lint_slot_edge(&mut issues, edge, &node_by_id);
        }
    }

    if has_main_flow_cycle(automation) {
        issues.push(ValidationIssue::error(
            None,
            "main flow has a cycle; only tree-shaped graphs can run",
        ));
    }

    for node in &automation.nodes {
        lint_node_data(&mut issues, automation, node);
    }

    issues
}

pub fn has_errors(issues: &[ValidationIssue]) -> bool {
    issues.iter().any(ValidationIssue::is_error)
}

/// Fill blank ids and lay out a pile-up at the origin so the canvas is readable.
pub fn prepare_for_upsert(automation: &mut Automation) {
    for (index, node) in automation.nodes.iter_mut().enumerate() {
        if node.id.trim().is_empty() {
            node.id = format!("n{}", index + 1);
        }
    }
    for (index, edge) in automation.edges.iter_mut().enumerate() {
        if edge.id.trim().is_empty() {
            edge.id = format!("e{}", index + 1);
        }
    }
    if needs_auto_layout(automation) {
        auto_layout(automation);
    }
}

pub fn needs_auto_layout(automation: &Automation) -> bool {
    automation.nodes.len() > 1
        && automation
            .nodes
            .iter()
            .all(|node| node.position.x == 0.0 && node.position.y == 0.0)
}

pub fn auto_layout(automation: &mut Automation) {
    let main: Vec<&FlowEdge> = automation
        .edges
        .iter()
        .filter(|edge| !is_slot_target_handle(edge.target_handle.as_deref()))
        .collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &main {
        outgoing
            .entry(edge.source.as_str())
            .or_default()
            .push(edge.target.as_str());
    }

    let mut depth: HashMap<String, usize> = HashMap::new();
    let mut queue = VecDeque::new();
    for node in &automation.nodes {
        if node.node_type.starts_with("trigger.") {
            depth.insert(node.id.clone(), 0);
            queue.push_back(node.id.clone());
        }
    }
    while let Some(id) = queue.pop_front() {
        let current = depth.get(&id).copied().unwrap_or(0);
        if let Some(nexts) = outgoing.get(id.as_str()) {
            for next in nexts {
                let candidate = current + 1;
                let existing = depth.get(*next).copied().unwrap_or(usize::MAX);
                if candidate < existing {
                    depth.insert((*next).to_string(), candidate);
                    queue.push_back((*next).to_string());
                }
            }
        }
    }

    let mut lanes: HashMap<usize, usize> = HashMap::new();
    let mut placed: HashMap<String, Vec2> = HashMap::new();
    let mut remaining: Vec<&FlowNode> = automation.nodes.iter().collect();
    remaining.sort_by(|a, b| {
        let da = depth.get(&a.id).copied().unwrap_or(99);
        let db = depth.get(&b.id).copied().unwrap_or(99);
        da.cmp(&db).then(a.id.cmp(&b.id))
    });
    for node in remaining {
        if node.node_type.starts_with("agent.") {
            continue;
        }
        let d = depth.get(&node.id).copied().unwrap_or(99);
        let lane = lanes.entry(d).or_insert(0);
        let y = *lane as f64 * LAYOUT_DY;
        *lane += 1;
        placed.insert(
            node.id.clone(),
            Vec2 {
                x: d as f64 * LAYOUT_DX,
                y,
            },
        );
    }

    let mut slot_lanes: HashMap<String, usize> = HashMap::new();
    for edge in &automation.edges {
        if !is_slot_target_handle(edge.target_handle.as_deref()) {
            continue;
        }
        let Some(agent_pos) = placed.get(&edge.target).cloned() else {
            continue;
        };
        let lane = slot_lanes.entry(edge.target.clone()).or_insert(0);
        let x_off = match edge.target_handle.as_deref() {
            Some("runtime") => -40.0,
            Some("context") => 40.0,
            Some("tool") => 120.0,
            Some("skill") => 200.0,
            _ => 0.0,
        };
        placed.insert(
            edge.source.clone(),
            Vec2 {
                x: agent_pos.x + x_off,
                y: agent_pos.y + LAYOUT_DY + (*lane as f64 * SLOT_DY),
            },
        );
        *lane += 1;
    }

    let mut leftover_lane = 0usize;
    for node in &mut automation.nodes {
        if let Some(pos) = placed.get(&node.id) {
            node.position = pos.clone();
            continue;
        }
        node.position = Vec2 {
            x: 3.0 * LAYOUT_DX,
            y: leftover_lane as f64 * LAYOUT_DY,
        };
        leftover_lane += 1;
    }
}

fn lint_slot_edge(
    issues: &mut Vec<ValidationIssue>,
    edge: &FlowEdge,
    node_by_id: &HashMap<&str, &FlowNode>,
) {
    let Some(target) = node_by_id.get(edge.target.as_str()) else {
        return;
    };
    if target.node_type != "action.agent" {
        issues.push(ValidationIssue::error(
            Some(edge.target.clone()),
            format!(
                "slot edge '{}' can only plug into action.agent, not '{}'",
                edge.id, target.node_type
            ),
        ));
    }
    let Some(source) = node_by_id.get(edge.source.as_str()) else {
        return;
    };
    let expected = match edge.target_handle.as_deref() {
        Some("runtime") => "agent.runtime",
        Some("context") => "agent.context",
        Some("tool") => "agent.tool",
        Some("skill") => "agent.skill",
        _ => return,
    };
    if source.node_type != expected {
        issues.push(ValidationIssue::error(
            Some(edge.source.clone()),
            format!(
                "slot '{}' must come from {}, not '{}'",
                edge.target_handle.as_deref().unwrap_or(""),
                expected,
                source.node_type
            ),
        ));
    }
}

fn has_main_flow_cycle(automation: &Automation) -> bool {
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &automation.edges {
        if is_slot_target_handle(edge.target_handle.as_deref()) {
            continue;
        }
        outgoing
            .entry(edge.source.as_str())
            .or_default()
            .push(edge.target.as_str());
    }
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    fn dfs<'a>(
        id: &'a str,
        outgoing: &HashMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
    ) -> bool {
        if visited.contains(id) {
            return false;
        }
        if !visiting.insert(id) {
            return true;
        }
        if let Some(nexts) = outgoing.get(id) {
            for next in nexts {
                if dfs(next, outgoing, visiting, visited) {
                    return true;
                }
            }
        }
        visiting.remove(id);
        visited.insert(id);
        false
    }
    automation
        .nodes
        .iter()
        .any(|node| dfs(&node.id, &outgoing, &mut visiting, &mut visited))
}

fn lint_node_data(issues: &mut Vec<ValidationIssue>, automation: &Automation, node: &FlowNode) {
    let id = Some(node.id.clone());
    match node.node_type.as_str() {
        "trigger.schedule" => {
            let schedule = node.data.get("schedule").cloned().unwrap_or(Value::Null);
            let kind = schedule
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("daily");
            if !SCHEDULE_KINDS.contains(&kind) {
                issues.push(ValidationIssue::error(
                    id.clone(),
                    format!("schedule.kind must be daily, weekdays, or interval (got '{kind}')"),
                ));
            }
            if kind != "interval" {
                let hour = json_u64(&schedule, "hour").unwrap_or(9);
                let minute = json_u64(&schedule, "minute").unwrap_or(0);
                if hour > 23 {
                    issues.push(ValidationIssue::error(id.clone(), "schedule.hour must be 0–23"));
                }
                if minute > 59 {
                    issues.push(ValidationIssue::error(id.clone(), "schedule.minute must be 0–59"));
                }
            } else {
                let minutes = json_u64(&schedule, "intervalMinutes").unwrap_or(0);
                if minutes == 0 {
                    issues.push(ValidationIssue::error(
                        id,
                        "schedule.intervalMinutes must be at least 1",
                    ));
                }
            }
        }
        "trigger.hotkey" => {
            let acc = node
                .data
                .get("hotkey")
                .and_then(|v| v.get("accelerator"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if acc.is_empty() {
                issues.push(ValidationIssue::warning(
                    id,
                    "hotkey trigger has an empty accelerator; it will not fire until one is set",
                ));
            }
        }
        "action.http" => {
            let http = node.data.get("http").cloned().unwrap_or(Value::Null);
            let method = http
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_ascii_uppercase();
            if !HTTP_METHODS.contains(&method.as_str()) {
                issues.push(ValidationIssue::error(
                    id.clone(),
                    format!("http.method must be one of GET/POST/PUT/PATCH/DELETE (got '{method}')"),
                ));
            }
            let url = http.get("url").and_then(Value::as_str).unwrap_or("").trim();
            if url.is_empty() {
                issues.push(ValidationIssue::error(id.clone(), "http.url is empty"));
            } else if !url.starts_with("http://") && !url.starts_with("https://") && !url.contains("{{")
            {
                issues.push(ValidationIssue::warning(
                    id,
                    "http.url should start with http:// or https://",
                ));
            }
        }
        "action.command" => {
            let cmd = node
                .data
                .get("command")
                .and_then(|v| v.get("command"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if cmd.is_empty() {
                issues.push(ValidationIssue::error(id, "command.command is empty"));
            }
        }
        "action.file" => {
            let file = node.data.get("file").cloned().unwrap_or(Value::Null);
            let op = file.get("op").and_then(Value::as_str).unwrap_or("write");
            if !FILE_OPS.contains(&op) {
                issues.push(ValidationIssue::error(
                    id.clone(),
                    format!("file.op must be read or write (got '{op}')"),
                ));
            }
            let path = file.get("path").and_then(Value::as_str).unwrap_or("").trim();
            if path.is_empty() {
                issues.push(ValidationIssue::error(id, "file.path is empty"));
            }
        }
        "action.clipboard" => {
            let op = node
                .data
                .get("clipboard")
                .and_then(|v| v.get("op"))
                .and_then(Value::as_str)
                .unwrap_or("copy");
            if !CLIPBOARD_OPS.contains(&op) {
                issues.push(ValidationIssue::error(
                    id,
                    format!("clipboard.op must be copy or read (got '{op}')"),
                ));
            }
        }
        "action.set" => {
            let fields = node
                .data
                .get("set")
                .and_then(|v| v.get("fields"))
                .and_then(Value::as_array);
            let has_key = fields.is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("key")
                        .and_then(Value::as_str)
                        .is_some_and(|k| !k.trim().is_empty())
                })
            });
            if !has_key {
                issues.push(ValidationIssue::warning(
                    id,
                    "set.fields is empty; this step will output {}",
                ));
            }
        }
        "logic.if" => {
            let op = node
                .data
                .get("if")
                .and_then(|v| v.get("op"))
                .and_then(Value::as_str)
                .unwrap_or("contains");
            if !IF_OPS.contains(&op) {
                issues.push(ValidationIssue::error(
                    id,
                    format!("if.op must be contains, equals, or notEmpty (got '{op}')"),
                ));
            }
        }
        "logic.switch" => {
            let cases = node
                .data
                .get("switch")
                .and_then(|v| v.get("cases"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if cases.is_empty() {
                issues.push(ValidationIssue::warning(
                    id.clone(),
                    "switch.cases is empty; every run will take the default handle",
                ));
            }
            let mut seen = HashSet::new();
            for case in &cases {
                let case_id = case
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("");
                if case_id.is_empty() {
                    issues.push(ValidationIssue::error(
                        id.clone(),
                        "a switch case is missing id",
                    ));
                    continue;
                }
                if !seen.insert(case_id.to_string()) {
                    issues.push(ValidationIssue::error(
                        id.clone(),
                        format!("duplicate switch case id '{case_id}'"),
                    ));
                }
                let op = case.get("op").and_then(Value::as_str).unwrap_or("equals");
                if !IF_OPS.contains(&op) {
                    issues.push(ValidationIssue::error(
                        id.clone(),
                        format!("switch case '{case_id}' has invalid op '{op}'"),
                    ));
                }
            }
        }
        "logic.delay" => {
            let seconds = json_u64(node.data.get("delay").unwrap_or(&Value::Null), "seconds")
                .unwrap_or(0);
            if seconds == 0 || seconds > 600 {
                issues.push(ValidationIssue::error(
                    id,
                    "delay.seconds must be between 1 and 600",
                ));
            }
        }
        "action.agent" => {
            let own_prompt = node
                .data
                .get("agent")
                .and_then(|v| v.get("prompt"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let has_context_slot = automation.edges.iter().any(|edge| {
                edge.target == node.id && edge.target_handle.as_deref() == Some("context")
            });
            if own_prompt.is_none() && !has_context_slot {
                issues.push(ValidationIssue::warning(
                    id,
                    "action.agent has no prompt and no agent.context slot; the step will fail at run time",
                ));
            }
        }
        _ => {}
    }
}

fn json_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
            .or_else(|| v.as_f64().and_then(|n| if n >= 0.0 { Some(n as u64) } else { None }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::types::{FlowEdge, SCHEMA_VERSION, Viewport};
    use serde_json::json;

    fn node(id: &str, node_type: &str) -> FlowNode {
        FlowNode {
            id: id.into(),
            node_type: node_type.into(),
            position: Vec2 { x: 0.0, y: 0.0 },
            data: json!({}),
        }
    }

    fn edge(source: &str, target: &str, handle: Option<&str>) -> FlowEdge {
        FlowEdge {
            id: format!("e-{source}-{target}"),
            source: source.into(),
            target: target.into(),
            source_handle: handle.map(|s| s.to_string()),
            target_handle: None,
        }
    }

    fn graph(nodes: Vec<FlowNode>, edges: Vec<FlowEdge>) -> Automation {
        Automation {
            schema_version: SCHEMA_VERSION,
            id: "a".into(),
            name: "t".into(),
            enabled: false,
            nodes,
            edges,
            viewport: Viewport::default(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn errors_of(automation: &Automation) -> Vec<String> {
        validate(automation)
            .into_iter()
            .filter(|issue| issue.is_error())
            .map(|issue| issue.message)
            .collect()
    }

    #[test]
    fn empty_graph_is_an_error() {
        let issues = validate(&graph(vec![], vec![]));
        assert!(has_errors(&issues));
        assert!(issues[0].message.contains("no nodes"));
    }

    #[test]
    fn unknown_type_and_missing_trigger() {
        let automation = graph(vec![node("n", "action.teleport")], vec![]);
        let messages = errors_of(&automation);
        assert!(messages.iter().any(|m| m.contains("unknown node type")));
        assert!(messages.iter().any(|m| m.contains("trigger.schedule")));
        assert!(messages.iter().any(|m| m.contains("at least one trigger")));
    }

    #[test]
    fn dangling_edge_is_an_error() {
        let automation = graph(
            vec![node("t", "trigger.manual")],
            vec![edge("t", "missing", None)],
        );
        let messages = errors_of(&automation);
        assert!(messages.iter().any(|m| m.contains("does not exist")));
    }

    #[test]
    fn slot_edge_must_target_action_agent() {
        let automation = graph(
            vec![
                node("t", "trigger.manual"),
                node("n", "action.notify"),
                node("r", "agent.runtime"),
            ],
            vec![FlowEdge {
                id: "e".into(),
                source: "r".into(),
                target: "n".into(),
                source_handle: Some("slot".into()),
                target_handle: Some("runtime".into()),
            }],
        );
        let messages = errors_of(&automation);
        assert!(messages.iter().any(|m| m.contains("action.agent")));
    }

    #[test]
    fn slot_source_type_must_match_handle() {
        let automation = graph(
            vec![
                node("t", "trigger.manual"),
                node("a", "action.agent"),
                node("c", "agent.context"),
            ],
            vec![FlowEdge {
                id: "e".into(),
                source: "c".into(),
                target: "a".into(),
                source_handle: Some("slot".into()),
                target_handle: Some("runtime".into()),
            }],
        );
        let messages = errors_of(&automation);
        assert!(messages.iter().any(|m| m.contains("agent.runtime")));
    }

    #[test]
    fn main_flow_cycle_is_an_error() {
        let automation = graph(
            vec![
                node("t", "trigger.manual"),
                node("a", "action.notify"),
                node("b", "action.notify"),
            ],
            vec![
                edge("t", "a", None),
                edge("a", "b", None),
                edge("b", "a", None),
            ],
        );
        assert!(errors_of(&automation)
            .iter()
            .any(|m| m.contains("cycle")));
    }

    #[test]
    fn slot_edges_do_not_count_as_cycles() {
        let automation = graph(
            vec![
                node("t", "trigger.manual"),
                node("a", "action.agent"),
                node("r", "agent.runtime"),
            ],
            vec![
                edge("t", "a", None),
                FlowEdge {
                    id: "slot".into(),
                    source: "r".into(),
                    target: "a".into(),
                    source_handle: Some("slot".into()),
                    target_handle: Some("runtime".into()),
                },
            ],
        );
        assert!(!has_errors(&validate(&automation)));
    }

    #[test]
    fn http_and_command_require_payloads() {
        let mut http = node("h", "action.http");
        http.data = json!({ "http": { "method": "TRACE", "url": "" } });
        let mut cmd = node("c", "action.command");
        cmd.data = json!({ "command": { "command": "  " } });
        let automation = graph(
            vec![node("t", "trigger.manual"), http, cmd],
            vec![edge("t", "h", None), edge("t", "c", None)],
        );
        let messages = errors_of(&automation);
        assert!(messages.iter().any(|m| m.contains("http.method")));
        assert!(messages.iter().any(|m| m.contains("http.url is empty")));
        assert!(messages.iter().any(|m| m.contains("command.command is empty")));
    }

    #[test]
    fn schedule_and_if_ops_are_checked() {
        let mut schedule = node("s", "trigger.schedule");
        schedule.data = json!({ "schedule": { "kind": "yearly", "hour": 9, "minute": 0 } });
        let mut iff = node("i", "logic.if");
        iff.data = json!({ "if": { "op": "regex", "value": "x" } });
        let automation = graph(vec![schedule, iff], vec![]);
        let messages = errors_of(&automation);
        assert!(messages.iter().any(|m| m.contains("schedule.kind")));
        assert!(messages.iter().any(|m| m.contains("if.op")));
    }

    #[test]
    fn delay_bounds_and_file_path() {
        let mut delay = node("d", "logic.delay");
        delay.data = json!({ "delay": { "seconds": 0 } });
        let mut file = node("f", "action.file");
        file.data = json!({ "file": { "op": "write", "path": "" } });
        let automation = graph(
            vec![node("t", "trigger.manual"), delay, file],
            vec![],
        );
        let messages = errors_of(&automation);
        assert!(messages.iter().any(|m| m.contains("delay.seconds")));
        assert!(messages.iter().any(|m| m.contains("file.path is empty")));
    }

    #[test]
    fn agent_without_prompt_is_a_warning() {
        let automation = graph(
            vec![node("t", "trigger.manual"), node("a", "action.agent")],
            vec![edge("t", "a", None)],
        );
        let issues = validate(&automation);
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.severity == "warning" && i.message.contains("prompt")));
    }

    #[test]
    fn auto_layout_spreads_a_pile_at_origin() {
        let mut automation = graph(
            vec![
                node("t", "trigger.manual"),
                node("a", "action.notify"),
                node("b", "action.notify"),
            ],
            vec![edge("t", "a", None), edge("a", "b", None)],
        );
        assert!(needs_auto_layout(&automation));
        auto_layout(&mut automation);
        let by_id: HashMap<_, _> = automation
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.position.clone()))
            .collect();
        assert_eq!(by_id["t"].x, 0.0);
        assert_eq!(by_id["a"].x, LAYOUT_DX);
        assert_eq!(by_id["b"].x, LAYOUT_DX * 2.0);
        assert!(needs_auto_layout(&automation) == false);
    }

    #[test]
    fn prepare_fills_blank_ids() {
        let mut automation = graph(
            vec![FlowNode {
                id: String::new(),
                node_type: "trigger.manual".into(),
                position: Vec2::default(),
                data: json!({}),
            }],
            vec![],
        );
        prepare_for_upsert(&mut automation);
        assert_eq!(automation.nodes[0].id, "n1");
    }
}
