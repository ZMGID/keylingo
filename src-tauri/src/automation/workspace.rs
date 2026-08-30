use std::fs;
use std::path::PathBuf;

use crate::native_tools::NativeToolWorkspace;

/// Synthetic conversation id for an automation agent run: `auto_{automation_id}`.
/// Native file tools resolve this to `{workingDirectory}/automations/{id}` without
/// creating a sidebar conversation.
pub fn conversation_id(automation_id: &str) -> String {
    format!("auto_{automation_id}")
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
