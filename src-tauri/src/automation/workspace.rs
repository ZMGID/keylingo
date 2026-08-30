use std::fs;
use std::path::PathBuf;

use crate::native_tools::NativeToolWorkspace;

/// Synthetic conversation id for an automation agent run: `auto_{automation_id}`.
/// Native file tools resolve this to `{workingDirectory}/automations/{id}` without
/// creating a sidebar conversation.
pub fn conversation_id(automation_id: &str) -> String {
    format!("auto_{automation_id}")
}

/// Persisted (archived) conversation used only when an Agent node runs an
/// external CLI — `run_external_cli_reply` needs a `conv_` id and a native
/// session file. Marked archived so it never appears in the sidebar.
/// Keyed by automation + node so two Agent steps don't share a CLI session.
pub fn external_conversation_id(automation_id: &str, node_id: &str) -> String {
    let auto = sanitize_conv_part(automation_id);
    let node = sanitize_conv_part(node_id);
    if node.is_empty() {
        format!("conv_auto_{auto}")
    } else {
        format!("conv_auto_{auto}_{node}")
    }
}

fn sanitize_conv_part(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

pub fn workspace_for_conversation(
    working_directory: &str,
    conversation_id: &str,
) -> Option<Result<NativeToolWorkspace, String>> {
    let rest = conversation_id.strip_prefix("auto_")?;
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('\\')
        || rest.contains("..")
        || !rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Some(Err("invalid automation workspace id".to_string()));
    }
    let root = working_directory.trim();
    if root.is_empty() {
        return Some(Ok(NativeToolWorkspace::standalone()));
    }
    let dir = PathBuf::from(root).join("automations").join(rest);
    if let Err(err) = fs::create_dir_all(&dir) {
        return Some(Err(format!("create automation workspace failed: {err}")));
    }
    Some(Ok(NativeToolWorkspace::conversation(dir)))
}

pub fn workbench_dir(working_directory: &str, automation_id: &str) -> Option<PathBuf> {
    let root = working_directory.trim();
    if root.is_empty() {
        return None;
    }
    let dir = PathBuf::from(root).join("automations").join(automation_id);
    let _ = fs::create_dir_all(&dir);
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_conversation_id_is_per_node_and_conv_prefixed() {
        let id = external_conversation_id("auto-1", "n-2");
        assert_eq!(id, "conv_auto_auto-1_n-2");
        assert!(id.starts_with("conv_"));
        assert_eq!(
            external_conversation_id("bad/id", "x y"),
            "conv_auto_badid_xy"
        );
    }
}
