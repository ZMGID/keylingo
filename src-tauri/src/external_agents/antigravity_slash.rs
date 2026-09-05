//! Kivio-maintained command catalog, verified with agy 1.1.26. No runtime discovery.
//! Reports cannot enter the NDJSON stream: agy emits ERROR and exits. Run them
//! separately without a conversation binding; native skill expansion stays in-stream.
use std::path::Path;
use std::time::Duration;

use crate::external_agents::spawn::{cli_command, fold_stderr};
use crate::external_agents::types::ExternalCliSlashCommand;
use crate::proc::NoConsoleWindow;

pub fn is_report(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "agents"
            | "changelog"
            | "config"
            | "settings"
            | "credits"
            | "effort"
            | "hooks"
            | "model"
            | "permissions"
            | "skills"
            | "usage"
            | "quota"
    )
}

pub fn is_terminal_only(name: &str) -> bool {
    matches!(
        name,
        "add-dir"
            | "artifact"
            | "btw"
            | "clear"
            | "new"
            | "context"
            | "copy"
            | "diff"
            | "exit"
            | "quit"
            | "fast"
            | "feedback"
            | "fork"
            | "branch"
            | "keybindings"
            | "logout"
            | "mcp"
            | "open"
            | "planning"
            | "rename"
            | "resume"
            | "switch"
            | "conversation"
            | "rewind"
            | "undo"
            | "statusline"
            | "tasks"
            | "title"
            | "voice"
            | "record"
    )
}

pub fn command_name(prompt: &str) -> Option<&str> {
    prompt
        .trim_start()
        .strip_prefix('/')?
        .split_whitespace()
        .next()
}

/// Keep the active model/effort/mode flags but never pass the stream protocol
/// or conversation id to a report process. Reports must not alter native history.
pub fn report_args(args: &[String], prompt: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if matches!(
            args[index].as_str(),
            "--input-format" | "--output-format" | "--conversation"
        ) {
            index += 2;
        } else {
            out.push(args[index].clone());
            index += 1;
        }
    }
    out.extend(["-p".into(), prompt.trim().into()]);
    out
}

pub async fn report(
    bin: &Path,
    cwd: &Path,
    args: &[String],
    prompt: &str,
) -> Result<String, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        cli_command(bin)
            .args(report_args(args, prompt))
            .current_dir(cwd)
            .no_console_window()
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Antigravity 命令超时（60s）".to_string())?
    .map_err(|e| format!("Antigravity 命令启动失败：{e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(fold_stderr(
            format!(
                "Antigravity 命令退出码：{}\n{stdout}",
                output.status.code().unwrap_or(-1)
            ),
            &stderr,
        ));
    }
    Ok(if stdout.is_empty() {
        stderr.trim().to_string()
    } else {
        stdout
    })
}

/// This catalog ships with Kivio. Opening `/` never starts agy or fetches a catalog.
pub fn builtin_commands() -> Vec<ExternalCliSlashCommand> {
    [
        ("help", "查看 Kivio 已适配的 Antigravity 命令"),
        ("agents", "列出可用的自定义 Agent"),
        ("changelog", "查看 Antigravity CLI 更新记录"),
        ("config", "查看 CLI 配置（只读）"),
        ("credits", "查看剩余 AI credits"),
        ("effort", "查看当前推理强度；切换请使用顶部强度菜单"),
        ("hooks", "查看 CLI hooks 配置（只读）"),
        ("model", "查看当前模型；切换请使用顶部模型菜单"),
        ("permissions", "查看 CLI 权限配置（只读）"),
        ("skills", "列出原生技能；使用 /技能名 任务 执行"),
        ("usage", "查看模型配额（别名 /quota）"),
    ]
    .into_iter()
    .map(|(name, description)| ExternalCliSlashCommand {
        name: name.into(),
        slash: format!("/{name}"),
        description: Some(description.into()),
        argument_hint: None,
    })
    .collect()
}

pub fn help_text() -> String {
    let mut text = builtin_commands()
        .into_iter()
        .map(|command| {
            format!(
                "{} — {}",
                command.slash,
                command.description.unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    text.push_str("\n\n技能：先用 /skills 查看 CLI 已加载的技能，再输入 /技能名 任务。配置查询不修改当前会话设置；终端专属命令请在 agy 中使用。");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_catalog_only_advertises_implemented_reports() {
        let commands = builtin_commands();
        let names: std::collections::HashSet<_> =
            commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names.len(), commands.len());
        assert!(commands
            .iter()
            .all(|c| is_report(&c.name) && !is_terminal_only(&c.name)));
        assert!(help_text().contains("/usage"));
        assert!(help_text().contains("/技能名"));
        assert_eq!(command_name("  /usage  "), Some("usage"));
        assert_eq!(command_name("hello /usage"), None);
    }

    #[test]
    fn report_launch_keeps_active_settings_without_stream_or_session() {
        let args = [
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--conversation",
            "native-id",
            "--model",
            "custom-model",
            "--effort",
            "high",
            "--add-dir",
            "C:\\my files",
        ]
        .map(str::to_string);
        assert_eq!(
            report_args(&args, "/model"),
            [
                "--model",
                "custom-model",
                "--effort",
                "high",
                "--add-dir",
                "C:\\my files",
                "-p",
                "/model"
            ]
        );
    }
}
