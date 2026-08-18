use serde::{Deserialize, Serialize};

use crate::settings::{ChatMcpServer, ChatNativeToolsConfig};

/// 保留名规避（wire 层别名）：部分上游把特定工具名当作**内部保留工具**拦截消化——
/// 实测 Cursor 系上游（grok-composer 等经 cursor2api 类代理）会把名为 `web_search` 的
/// 工具调用吞掉（模型明确决定调用，流里却什么都不返回，表现为空响应）。全局把这类
/// 内置工具名映射成无歧义的 wire 别名：请求里声明别名、系统提示词渲染别名，收到
/// 别名调用后经 `match_tool_call`（匹配 `openai_tool_name()`）自然映射回内部工具执行。
/// 对正经 provider 无副作用（别名同样是合法工具名）。
const RESERVED_WIRE_ALIASES: &[(&str, &str)] = &[("web_search", "search_web")];

/// 内部工具名 → wire 别名（无命中原样返回）。
pub fn apply_reserved_wire_alias(name: &str) -> String {
    RESERVED_WIRE_ALIASES
        .iter()
        .find(|(from, _)| *from == name)
        .map(|(_, to)| (*to).to_string())
        .unwrap_or_else(|| name.to_string())
}

/// wire 别名 → 内部工具名（无命中原样返回）。供按模型侧函数名反查内部工具的路径使用
/// （如 disabled-tool 反馈）。
pub fn resolve_reserved_wire_alias(name: &str) -> &str {
    RESERVED_WIRE_ALIASES
        .iter()
        .find(|(_, to)| *to == name)
        .map(|(from, _)| *from)
        .unwrap_or(name)
}

/// 旧工具名 → 现工具名。工具被移除/合并/改名后，旧名仍需能路由到现工具，覆盖两条输入
/// 路径：① 模型发出的旧名工具调用（`match_tool_call`），② persona/skill 存储的工具白名单
/// （`tool_matches_recommended_name`）。
///
/// **方向与 `RESERVED_WIRE_ALIASES` 相反**：wire 别名是"内部名 → 模型可见名"（改对外暴露）；
/// 这里是"旧输入名 → 现内部名"（把历史输入规整到现工具），**不参与**工具声明/提示词渲染。
const LEGACY_TOOL_ALIASES: &[(&str, &str)] = &[
    ("ls", "read"),                     // ls 并入 read（read 现在可读目录）
    ("find", "glob"),                   // find 改名 glob
    ("list_background", "bash_output"), // list_background 并入 bash_output（无 job_id=列表）
    ("todo_update", "todo_write"),      // todo_update 并入 todo_write（整表替换）
    ("skill_activate", "skill"),        // skill_activate 改名 skill（合并 read_file/run_script 后）
];

/// 旧工具名规整为现工具名（无命中原样返回）。
pub fn canonical_tool_name(name: &str) -> &str {
    LEGACY_TOOL_ALIASES
        .iter()
        .find(|(from, _)| *from == name)
        .map(|(_, to)| *to)
        .unwrap_or(name)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatToolDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub input_schema: serde_json::Value,
    pub sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
}

impl ChatToolDefinition {
    pub fn openai_tool_name(&self) -> String {
        wire_tool_name(
            &self.source,
            &self.name,
            &self.id,
            self.server_id.as_deref(),
        )
    }

    pub fn to_openai_tool(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.openai_tool_name(),
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }

    pub fn read_only_hint(&self) -> Option<bool> {
        annotation_bool(self.annotations.as_ref(), "readOnlyHint")
    }

    pub fn destructive_hint(&self) -> Option<bool> {
        annotation_bool(self.annotations.as_ref(), "destructiveHint")
    }

    pub fn open_world_hint(&self) -> Option<bool> {
        annotation_bool(self.annotations.as_ref(), "openWorldHint")
    }

    pub fn is_read_only_tool(&self) -> bool {
        if self.source == "mcp" {
            return self.read_only_hint() == Some(true)
                && self.destructive_hint() != Some(true)
                && self.open_world_hint() != Some(true);
        }
        // Native read-only metadata lives in the static registry
        // (mcp/native_registry.rs). Note: this set includes memory_read and
        // memory_search, which are read-only but deliberately not parallel-safe.
        self.source == "native"
            && super::native_registry::find_entry(&self.name).is_some_and(|entry| entry.read_only)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallResult {
    pub content: String,
    pub is_error: bool,
    pub raw: serde_json::Value,
    #[serde(default)]
    pub artifacts: Vec<ChatToolArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<serde_json::Value>,
    /// Extra user-role messages (OpenAI wire shape) to append to the
    /// conversation right after this tool's result message. Used by `read` to
    /// feed an image to a vision-capable model: a tool result can only carry
    /// text, so the actual image rides here as a follow-up user message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow_up_user_messages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(alias = "mimeType")]
    pub mime_type: String,
    #[serde(alias = "dataUrl")]
    pub data_url: String,
    #[serde(default, alias = "sizeBytes")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonRunResult {
    pub content: String,
    #[serde(alias = "isError")]
    pub is_error: bool,
    #[serde(default)]
    pub artifacts: Vec<ChatToolArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
    #[serde(default, rename = "outputSchema")]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub annotations: Option<serde_json::Value>,
}

pub fn tool_definition_from_mcp(server: &ChatMcpServer, tool: McpTool) -> ChatToolDefinition {
    let id = format!("mcp__{}__{}", server.id, tool.name);
    let sensitive = mcp_tool_requires_confirmation(&tool);
    ChatToolDefinition {
        id,
        name: tool.name.clone(),
        description: if tool.description.trim().is_empty() {
            format!("MCP tool {}", tool.name)
        } else {
            tool.description
        },
        source: "mcp".to_string(),
        server_id: Some(server.id.clone()),
        server_name: Some(server.name.clone()),
        input_schema: if tool.input_schema.is_null() {
            serde_json::json!({ "type": "object", "properties": {} })
        } else {
            tool.input_schema
        },
        sensitive,
        annotations: tool.annotations,
        output_schema: tool.output_schema,
    }
}

fn annotation_bool(annotations: Option<&serde_json::Value>, key: &str) -> Option<bool> {
    let annotations = annotations?;
    let snake_key = to_snake_case(key);
    annotations
        .get(key)
        .or_else(|| annotations.get(&snake_key))
        .and_then(|value| value.as_bool())
}

fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if idx > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn mcp_tool_requires_confirmation(_tool: &McpTool) -> bool {
    // MCP annotations are server-provided, untrusted hints. They may be displayed to the user,
    // but must never relax Kivio's approval boundary. A user-owned approval policy (for example
    // the explicit global `auto` policy) is the only way to bypass per-call MCP confirmation.
    true
}

pub fn native_web_search_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__web_search".to_string(),
        name: "web_search".to_string(),
        description: "Search the web for current facts and return source snippets.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["query"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_skill_activate_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "skill__activate".to_string(),
        name: "skill".to_string(),
        description: "Load a specialized skill when the task at hand matches one of the skills listed in the system prompt. Injects the skill's instructions and resources into the current conversation — the output may contain detailed workflow guidance plus references to scripts and files in the skill directory (read them with `read`, run scripts with `run_python` or `run_command`). The skill name must match one listed in available_skills.".to_string(),
        source: "skill".to_string(),
        server_id: None,
        server_name: Some("Skill".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill name from available_skills"
                }
            },
            "required": ["name"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_skill_tools() -> Vec<ChatToolDefinition> {
    vec![native_skill_activate_tool()]
}

pub fn native_read_file_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__read_file".to_string(),
        name: "read".to_string(),
        description: "Read a local file or directory. For a file: text is line-numbered as `N<TAB>line` for easy reference; the numbers are display-only and are NOT part of the file — never include them in edit old_string. Output is capped at 2000 lines or 50KB, whichever is hit first, so a single read can never flood the context; when the cap or your own limit stops the read early the result says so and reports total_lines and next_offset — continue with offset until you have what you need. Optional offset/limit select a 1-based line window (the cap still applies on top). For a directory path: returns its entries (folded in the former `ls` tool); offset/limit are ignored. Image files (png/jpg/webp/…) are also supported: the image is shown to you directly when your model has vision, otherwise it is described or OCR'd to text — so you can `read` screenshots and photos by path. For PDF/Word/Excel, use the matching skill instead.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to read. Relative paths resolve from the project root/current workspace; absolute and ~/ paths are also accepted when allowed by workspace mode." },
                "offset": { "type": "integer", "description": "1-based start line (optional)" },
                "limit": { "type": "integer", "description": "Max lines to return (optional)" }
            },
            "required": ["path"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

/// Directory listing tool def. No longer part of the chat native tool set (chat's
/// `read` now lists directories directly), but still used by the Kivio Code surface,
/// which keeps a dedicated `ls` in its own tool list.
pub fn native_list_dir_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__list_dir".to_string(),
        name: "ls".to_string(),
        description: "List files and directories in a directory. Omit path (or pass \".\") to list the current working directory; relative paths resolve from it. Do not guess or invent an absolute path, and never translate/\"correct\" directory names — pass an absolute or ~/ path only when the user gave one or an earlier tool returned it.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to list. Defaults to the current working directory; relative paths resolve from it. Do not fabricate an absolute path." },
                "include_hidden": { "type": "boolean", "description": "Include dotfiles and hidden entries" },
                "max_entries": { "type": "integer", "description": "Maximum entries to return, default 200, max 500" }
            }
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_search_files_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__search_files".to_string(),
        name: "grep".to_string(),
        description: "Search text in a file or under a directory. By default `query` is a literal substring; set regex=true to treat it as a regular expression. If you already know the exact file, pass that file path directly; for broader searches, pass a directory and use `glob` to narrow the scope. Relative paths resolve from the project root; respects .gitignore and skips common dependency/build folders (node_modules, target, dist, …).".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Text to search for (alias: pattern). Literal substring by default; a regular expression when regex=true." },
                "pattern": { "type": "string", "description": "Alias for query." },
                "path": { "type": "string", "description": "File or directory path, defaults to project root/current workspace" },
                "regex": { "type": "boolean", "description": "Treat query as a regular expression, default false (literal substring)" },
                "case_sensitive": { "type": "boolean", "description": "Case-sensitive matching, default false" },
                "include_hidden": { "type": "boolean", "description": "Include dotfiles and hidden entries" },
                "glob": { "type": "string", "description": "Only search files whose relative path or name matches this glob. Supports brace expansion: \"*.{py,ts}\" matches both .py and .ts files. Examples: \"*.rs\", \"src/**/*.ts\", \"*.{py,ts,js}\"" },
                "output_mode": { "type": "string", "enum": ["content", "files_with_matches", "count"], "description": "content: matching lines (default); files_with_matches: list of matching file paths; count: per-file match counts" },
                "context": { "type": "integer", "description": "Number of context lines to include before and after each match (content mode only), default 0" },
                "max_results": { "type": "integer", "description": "Maximum results to return, default 100, max 1000" }
            },
            "required": []
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_glob_files_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__glob_files".to_string(),
        name: "glob".to_string(),
        description: "Find files/directories under a directory by glob pattern such as \"src/**/*.tsx\". Relative paths resolve from the project root; respects .gitignore.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern with *, ?, and ** support" },
                "path": { "type": "string", "description": "Directory path to search, defaults to project root/current workspace" },
                "include_hidden": { "type": "boolean", "description": "Include dotfiles and hidden entries" },
                "max_results": { "type": "integer", "description": "Maximum paths to return, default 200, max 500" }
            },
            "required": ["pattern"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_write_file_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__write_file".to_string(),
        name: "write".to_string(),
        description: "Write a full text file: create it if missing, overwrite it if it exists. Use this when the user explicitly asks to save/write/create a local file or gives a target path; for small changes to an existing file prefer edit. Do not call it just because the user asked for a code block or inline code — answer directly instead. Returns structured file mutation metadata including diff stats.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Project-relative path in project mode, otherwise an explicitly requested absolute/home/~/ path" },
                "content": { "type": "string", "description": "Full text content to save" }
            },
            "required": ["path", "content"]
        }),
        sensitive: true,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_edit_file_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__edit_file".to_string(),
        name: "edit".to_string(),
        description: "Edit a file with one or more exact text replacements in a single call. Each edit's old_string must match a unique, contiguous region of the current file (copy it from read output WITHOUT the leading line-number prefix); if a snippet appears more than once, extend it with surrounding context. Edits apply in order. Prefer this over write for changes to existing files. Returns structured file mutation metadata including diff stats.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "edits": {
                    "type": "array",
                    "description": "One or more replacements, applied in order. Each old_string must occur exactly once in the current file.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string", "description": "Exact text to replace (unique in the file)" },
                            "new_string": { "type": "string", "description": "Replacement text" }
                        },
                        "required": ["old_string", "new_string"]
                    },
                    "minItems": 1
                }
            },
            "required": ["path", "edits"]
        }),
        sensitive: true,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_run_command_tool() -> ChatToolDefinition {
    // 描述按实际选中的 shell 动态生成(opencode #16479 教训:shell 自动检测对模型
    // 不可见 → 模型猜错语法)。Windows 检出 Git Bash 时追加 bash 语法 + 正斜杠路径
    // 提示;PowerShell 回落 / 非 Windows 时该 hint 为空串,描述与旧版逐字一致。
    let shell_hint = crate::native_tools::run_command_shell_hint();
    ChatToolDefinition {
        id: "native__run_command".to_string(),
        name: "bash".to_string(),
        description: format!("Run a host shell command (build, test, etc.).{shell_hint} In a project conversation, the command starts from the bound project root by default; any explicit cwd is only a startup directory and is validated as workspace-local. Do not use `cd path && command` when the path contains spaces—pass `cwd` and run only the remaining command. Do not combine `cwd` with a leading `cd ... &&` prefix. Long-running dev servers such as `npm run dev`, `npm run tauri dev`, and `vite` are started in the background automatically and return immediately with a pid. This is a sensitive host-shell capability, not the same boundary as the file tools: obey user constraints and explain or seek confirmation before cross-directory, destructive, network, or environment-changing commands. A non-zero exit code is returned as a tool error with stdout/stderr. Do not use pip to bypass run_python sandbox failures; host Python package installs require an explicit user request and allow_host_python_package_install=true."),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command" },
                "cwd": { "type": "string", "description": "Working directory (required when the path contains spaces; do not use `cd ... &&` for that)" },
                "background": { "type": "boolean", "description": "Run in background and return immediately (auto-enabled for common dev servers)" },
                "timeout_ms": { "type": "integer", "description": "Timeout in ms (optional)" },
                "allow_host_python_package_install": { "type": "boolean", "description": "Only true when the user explicitly asked to modify the host Python environment; installs must use --user or a virtual environment." }
            },
            "required": ["command"]
        }),
        sensitive: true,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_bash_output_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__bash_output".to_string(),
        name: "bash_output".to_string(),
        description: "Inspect background commands started by bash (background:true). With a job_id: returns that job's captured stdout/stderr since since_offset (default 0), the current status (running / exited with exit_code / killed / error), and next_offset for incremental reads. With NO job_id: lists all background commands tracked in this app session (job_id, status, command, working directory, age) — background commands survive across turns until killed or the app exits. After dispatching a background command, do NOT poll immediately — keep working, then poll a bounded number of times (≤20). Always refresh once with bash_output before reporting a background command's result to the user.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "The job_id returned when the background command was started. Omit to list all tracked background jobs instead." },
                "since_offset": { "type": "integer", "description": "Byte offset to read from (use next_offset from the previous bash_output call for incremental reads; default 0)" }
            }
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_kill_background_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__kill_background".to_string(),
        name: "kill_background".to_string(),
        description: "Stop a background command started by bash (background:true) by killing its process group. Pass the job_id. Use this to stop a dev server or other long-running background process when you are done with it; otherwise it keeps running until the app exits.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "The job_id of the background command to kill" }
            },
            "required": ["job_id"]
        }),
        sensitive: true,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_save_assistant_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__save_assistant".to_string(),
        name: "save_assistant".to_string(),
        description: "Create a new Kivio assistant (专家). ONLY available while building an assistant by chat, and only call it after you have restated the full config and the user confirmed. system_prompt is the assistant's own instructions (write it in the user's language). mcp_server_ids and skill_ids MUST be chosen from the available lists given in your builder instructions — use the exact ids, never invent them; leave a list empty if none apply. Returns the new assistant id.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Assistant display name (1-64 chars)" },
                "description": { "type": "string", "description": "Short one-line description (optional)" },
                "icon": { "type": "string", "description": "Optional short icon label/emoji" },
                "color": { "type": "string", "description": "Optional hex color like #6A8FBD" },
                "system_prompt": { "type": "string", "description": "The assistant's own system instructions" },
                "mcp_server_ids": { "type": "array", "items": { "type": "string" }, "description": "Allowed MCP server ids (exact ids from the available list; empty = none)" },
                "skill_ids": { "type": "array", "items": { "type": "string" }, "description": "Allowed skill ids (exact ids from the available list; empty = none)" }
            },
            "required": ["name", "system_prompt"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_present_artifacts_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__present_artifacts".to_string(),
        name: "present_artifacts".to_string(),
        description: "Show files or images in the chat. You must call this when the user asks to show, preview, attach, or send a file; reading or describing a file does not display it. Pass artifact_ids for files this conversation generated, or paths for files that already exist on disk — never both for the same file, and never invent a path for a generated file. Unselected files remain hidden.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "artifact_ids": {
                    "type": "array",
                    "description": "IDs of files generated in this conversation (e.g. images from mixer_generate_image). These have no filesystem path — do not also list them in paths.",
                    "items": { "type": "string", "minLength": 1 },
                    "minItems": 1,
                    "maxItems": 16
                },
                "paths": {
                    "type": "array",
                    "description": "Paths of files that already exist on disk. Only for files you read or wrote yourself; never for generated artifacts.",
                    "items": { "type": "string", "minLength": 1 },
                    "minItems": 1,
                    "maxItems": 16
                },
                "caption": {
                    "type": "string",
                    "description": "Optional short caption",
                    "maxLength": 300
                }
            },
            // 不用顶层 anyOf 表达“二选一必填”：grok/Vertex/Anthropic 都会拒或需剥离，
            // call_present_artifacts 在运行时校验，模型侧靠 description 提示即可。
            "additionalProperties": false
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_run_python_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__run_python".to_string(),
        name: "run_python".to_string(),
        description: "Execute Python code in a Pyodide sandbox with no direct host filesystem access. Use for computation, statistics, data analysis (numpy/pandas), reading and analyzing documents (PDF/XLSX), charts and plots (matplotlib), sandbox-compatible package installs, and generating files that REQUIRE a Python library to produce (formatted XLSX, PDF, rendered images). Its generated files are registered as artifacts but are not shown automatically; call present_artifacts with the returned artifact IDs when the user should see them. Do NOT use run_python merely to write a file from content you already have — for that, use write_file (to the current workbench, or to an explicit path requested by the user), then call present_artifacts only for files that should be shown in chat. Bundled packages auto-load on import: numpy, matplotlib, pandas, pillow, seaborn, openpyxl, xlrd, et_xmlfile, pypdf, micropip. Prefer plain import statements; do not write await micropip.install in sync code. To analyze local files, pass paths in files using the same syntax as the read tool; in project conversations these resolve from the project root by default. Mounted paths appear in KIVIO_INPUT_FILES. Save outputs to relative filenames in the Pyodide cwd (e.g. report.xlsx, chart.png, summary.csv); do not write host paths such as /Users or ~/Desktop inside Python. Kivio auto-captures images plus csv/json/md/txt/html/xlsx artifacts into the current default workbench and returns artifact IDs; use present_artifacts to place selected artifacts in the response. In chart text (titles, labels, legends, annotations) use only Latin and Chinese/Japanese/Korean characters; the sandbox bundles only a CJK+Latin font and has no emoji or symbol fonts, so emoji and decorative glyphs render as empty boxes—omit them. stdout/stderr are returned.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "Python source code" },
                "files": {
                    "type": "array",
                    "description": "Optional readable local file paths to copy into the Pyodide filesystem for this run",
                    "items": { "type": "string" },
                    "maxItems": 8
                },
                "timeout_ms": { "type": "integer", "description": "Timeout in ms (optional, max 300000)" }
            },
            "required": ["code"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_memory_read_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__memory_read".to_string(),
        name: "memory_read".to_string(),
        description: "Read Kivio Chat memory. L1 is online memory already injected when memory is enabled; use this mainly to inspect exact L1 text or read L2 long-term memory by exact query/heading.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "layer": {
                    "type": "string",
                    "enum": ["l1", "l2"],
                    "description": "Memory layer to read"
                },
                "query": {
                    "type": "string",
                    "description": "Optional exact text/heading query. Especially useful for L2."
                },
                "maxBytes": {
                    "type": "integer",
                    "description": "Maximum bytes returned to the model"
                }
            },
            "required": ["layer"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_memory_modify_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__memory_modify".to_string(),
        name: "memory_modify".to_string(),
        description: "Modify Kivio Chat memory. Use for adding, replacing, removing, or archiving durable user-approved memory. L1 is short online memory limited to 5000 bytes; L2 is long-term memory that is never auto-loaded.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "layer": {
                    "type": "string",
                    "enum": ["l1", "l2"],
                    "description": "Target memory layer"
                },
                "operation": {
                    "type": "string",
                    "enum": ["append", "replace", "remove", "archive"],
                    "description": "Modification operation"
                },
                "content": {
                    "type": "string",
                    "description": "New Markdown content for append/replace, or optional archived content"
                },
                "oldText": {
                    "type": "string",
                    "description": "Exact unique snippet for replace/remove/archive"
                },
                "heading": {
                    "type": "string",
                    "description": "Optional L2 heading to append/archive under"
                },
                "archiveMode": {
                    "type": "string",
                    "enum": ["move", "copy"],
                    "description": "archive only; default is move"
                }
            },
            "required": ["layer", "operation"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_memory_search_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__memory_search".to_string(),
        name: "memory_search".to_string(),
        description: "Search Kivio Chat long-term memory (L2) by keywords and get the most relevant entries back as heading + snippet. Prefer this over memory_read when you are not sure of the exact L2 heading: memory_read needs an exact heading/text match, while memory_search ranks sections by query-token overlap.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords to search for in long-term memory"
                },
                "maxResults": {
                    "type": "integer",
                    "description": "Maximum number of matching entries to return (default 5, max 20)"
                },
                "layer": {
                    "type": "string",
                    "enum": ["l1", "l2"],
                    "description": "Memory layer to search; defaults to l2 (L1 is small and already injected)"
                }
            },
            "required": ["query"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn mixer_generate_image_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "mixer__generate_image".to_string(),
        name: "mixer_generate_image".to_string(),
        description: "Generate image artifacts from a text prompt using the Mixer image generation model configured in Settings.".to_string(),
        source: "mixer".to_string(),
        server_id: None,
        server_name: Some("Mixer".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Detailed image generation prompt"
                },
                "size": {
                    "type": "string",
                    "enum": ["auto", "1024x1024", "1024x1536", "1536x1024"],
                    "description": "Optional output size. Use auto unless the user asked for a square, portrait, or landscape image."
                },
                "quality": {
                    "type": "string",
                    "enum": ["auto", "low", "medium", "high"],
                    "description": "Optional quality setting"
                },
                "n": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 4,
                    "description": "Number of images to generate"
                }
            },
            "required": ["prompt"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

pub fn native_web_fetch_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__web_fetch".to_string(),
        name: "web_fetch".to_string(),
        description: "Fetch readable text from an HTTPS URL (HTML is stripped to plain text)."
            .to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "HTTPS URL" },
                "reader_fallback": {
                    "type": "boolean",
                    "description": "Whether to try a hosted reader fallback when direct fetch fails or returns too little readable text. Defaults to true."
                }
            },
            "required": ["url"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

/// `knowledge_search` — retrieve passages from the user's knowledge bases
/// (RAG). Read-only. Returns passages each tagged with a `[n]` citation marker.
pub fn native_knowledge_search_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__knowledge_search".to_string(),
        name: "knowledge_search".to_string(),
        description: "Search the user's knowledge base(s) for passages relevant to a query and return them with citation markers. Use this whenever the question may be answered by the user's uploaded documents. Each returned passage is prefixed with a [n] marker and its source; when you use a passage, cite it inline as [n]. If no relevant passage is returned, say you don't have that information in the knowledge base instead of guessing. This searches only the libraries the user attached to the current conversation; if none are attached it returns nothing.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to look for. Use a focused natural-language query."
                },
                "kb_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional: restrict the search to these knowledge base ids. Defaults to the libraries attached to the current conversation; if none are attached, the search returns nothing."
                },
                "top_k": {
                    "type": "integer",
                    "description": "Optional: number of passages to return (default 5)."
                }
            },
            "required": ["query"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

/// `advisor` — consult a stronger model (executor-advisor pattern). The main
/// loop model calls this on demand when it is stuck, repeatedly failing, or
/// facing a significant design decision; the advisor model configured under
/// Mixer answers in a single shot (no tools, no recursion). Read-only.
pub fn native_advisor_tool() -> ChatToolDefinition {
    ChatToolDefinition {
        id: "native__advisor".to_string(),
        name: "advisor".to_string(),
        description: "Consult a stronger advisor model for guidance. Use this when you are stuck, have failed the same approach repeatedly, or face a significant design/architecture decision — NOT for routine steps you can handle yourself. Pass the specific question and enough context (relevant code, what you already tried, constraints). Returns the advisor's diagnosis and direction; you remain responsible for carrying it out.".to_string(),
        source: "native".to_string(),
        server_id: None,
        server_name: Some("Kivio".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The specific question or problem you want advice on."
                },
                "context": {
                    "type": "string",
                    "description": "Optional but recommended: relevant code, constraints, error messages, and what you have already tried. The advisor sees only what you pass here."
                }
            },
            "required": ["question"]
        }),
        sensitive: false,
        annotations: None,
        output_schema: None,
    }
}

/// Builtin native tool exposure: iterates the static registry in
/// `mcp/native_registry.rs` (declaration order = model-facing order).
pub fn list_native_builtin_tool_defs(
    native: &ChatNativeToolsConfig,
    web_search_configured: bool,
    memory_enabled: bool,
) -> Vec<ChatToolDefinition> {
    super::native_registry::NATIVE_TOOLS
        .iter()
        .filter(|entry| (entry.enabled)(native, web_search_configured, memory_enabled))
        .map(|entry| (entry.def)())
        .collect()
}

/// OpenAI / Gemini / Anthropic function names share a 64-char cap
/// (`^[a-zA-Z0-9_-]{1,64}$`). Naive left-truncate turns
/// `mcp__<uuid>__ui_to_artifact` and `mcp__<uuid>__ui_diff_check` into the
/// same `...-ui` and Gemini silently drops the rest.
const MAX_WIRE_TOOL_NAME: usize = 64;
const WIRE_NAME_HASH_LEN: usize = 8;
/// Server ids longer than this eat the 64-char budget; replace with a hash.
const LONG_MCP_SERVER_ID: usize = 16;

/// Shared wire-name builder for [`ChatToolDefinition`] and `ModelTool`.
pub fn wire_tool_name(source: &str, name: &str, id: &str, server_id: Option<&str>) -> String {
    match source {
        // Native and Skill tools are model-facing APIs owned by Kivio. Keep their names
        // aligned with the system prompt so models can call exactly what we instruct.
        "native" | "skill" | "mixer" => {
            apply_reserved_wire_alias(&sanitize_openai_tool_name(name))
        }
        _ => sanitize_openai_tool_name(&compact_mcp_tool_id(id, server_id)),
    }
}

/// `mcp__<very-long-server-id>__<tool>` → `mcp__<8-hex>__<tool>` so the
/// distinguishing tool suffix survives the 64-char cap. Short / already-compact
/// ids are left alone. `sanitize_openai_tool_name` still hash-suffixes if the
/// compact form itself exceeds 64.
fn compact_mcp_tool_id(id: &str, server_id: Option<&str>) -> String {
    let Some(server_id) =
        server_id.filter(|s| !s.is_empty() && s.chars().count() > LONG_MCP_SERVER_ID)
    else {
        return id.to_string();
    };
    let prefix = format!("mcp__{server_id}__");
    let Some(tool) = id.strip_prefix(&prefix).filter(|t| !t.is_empty()) else {
        return id.to_string();
    };
    format!("mcp__{}__{tool}", short_stable_hash(server_id))
}

fn short_stable_hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .flat_map(|byte| [hex_digit(byte >> 4), hex_digit(byte & 0x0f)])
        .take(WIRE_NAME_HASH_LEN)
        .collect()
}

fn hex_digit(nibble: u8) -> char {
    b"0123456789abcdef"[nibble as usize] as char
}

pub fn sanitize_openai_tool_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        return "tool".to_string();
    }
    if trimmed.chars().count() <= MAX_WIRE_TOOL_NAME {
        return trimmed.to_string();
    }
    // Left-truncate used to collide (issue #33). Keep a stable unique suffix:
    // `<prefix>_<8-hex>` always 64 chars, deterministic, legal on the wire.
    let digest = short_stable_hash(trimmed);
    let prefix_len = MAX_WIRE_TOOL_NAME - 1 - WIRE_NAME_HASH_LEN;
    let prefix: String = trimmed.chars().take(prefix_len).collect();
    format!("{prefix}_{digest}")
}

pub fn looks_sensitive_tool(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "write", "delete", "remove", "exec", "shell", "command", "run", "update", "patch", "move",
        "rename", "create", "save", "upload", "publish", "replace", "modify", "edit", "insert",
        "drop", "truncate", "grant", "revoke", "deploy", "apply",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_tool_detection_covers_common_write_verbs() {
        for name in ["save_file", "uploadAsset", "publish_page", "replace_rows"] {
            assert!(
                looks_sensitive_tool(name),
                "{name} should require confirmation"
            );
        }
        assert!(!looks_sensitive_tool("read_file"));
        assert!(!looks_sensitive_tool("web_search"));
    }

    #[test]
    fn skill_and_native_tools_use_prompt_facing_names() {
        assert_eq!(native_skill_activate_tool().openai_tool_name(), "skill");
        // 保留名规避：native web_search 在 wire/prompt 上声明为别名 search_web。
        assert_eq!(native_web_search_tool().openai_tool_name(), "search_web");
    }

    #[test]
    fn skill_activate_legacy_name_maps_to_skill() {
        assert_eq!(canonical_tool_name("skill_activate"), "skill");
    }

    #[test]
    fn native_skill_tools_expose_single_tool() {
        let tools = native_skill_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "skill");
    }

    #[test]
    fn builtin_skill_tools_are_not_marked_sensitive() {
        assert!(!native_skill_activate_tool().sensitive);
    }

    #[test]
    fn native_file_and_web_tools_have_expected_sensitivity() {
        assert!(!native_read_file_tool().sensitive);
        assert!(!native_web_fetch_tool().sensitive);
        assert!(!native_run_python_tool().sensitive);
        assert!(!native_memory_read_tool().sensitive);
        assert!(!native_memory_modify_tool().sensitive);
        assert!(native_write_file_tool().sensitive);
        assert!(native_edit_file_tool().sensitive);
        assert!(native_run_command_tool().sensitive);
    }

    #[test]
    fn file_tool_path_descriptions_are_scope_specific() {
        let read_schema = native_read_file_tool().input_schema;
        let grep = native_search_files_tool();
        let find = native_glob_files_tool();

        assert!(read_schema["properties"]["path"]["description"]
            .as_str()
            .unwrap()
            .contains("File path"));
        // read 现在也列目录，描述里应提到目录
        assert!(native_read_file_tool().description.contains("directory"));
        assert!(grep.description.contains("file or under a directory"));
        assert!(grep.description.contains("pass that file path directly"));
        assert!(grep.input_schema["properties"]["path"]["description"]
            .as_str()
            .unwrap()
            .contains("File or directory path"));
        assert!(find.input_schema["properties"]["path"]["description"]
            .as_str()
            .unwrap()
            .contains("Directory"));
    }

    #[test]
    fn write_file_tool_description_discourages_inline_code_requests() {
        let tool = native_write_file_tool();

        assert!(tool.description.contains("explicitly asks"));
        assert!(tool.description.contains("code block"));
        assert!(tool.description.contains("prefer edit"));
        assert!(tool
            .description
            .contains("structured file mutation metadata"));
    }

    #[test]
    fn run_python_tool_description_scopes_to_compute_deliverables() {
        let tool = native_run_python_tool();

        // Compute/analysis/chart power is retained.
        assert!(tool.description.contains("computation"));
        assert!(tool.description.contains("data analysis"));
        assert!(tool.description.contains("charts and plots"));
        assert!(tool.description.contains("REQUIRE a Python library"));
        assert!(tool.description.contains("report.xlsx"));
        assert!(tool.description.contains("Kivio auto-captures"));
        // The old "just write a file" catch-all language is gone, and it now
        // points at write for content you already have.
        assert!(!tool
            .description
            .contains("user-requested chat deliverable files"));
        assert!(!tool
            .description
            .contains("does not need to mention Python or run_python"));
        // No reference to the removed deliver_file tool; deliverables are a
        // path-driven channel via write into the conversation workbench.
        assert!(!tool.description.contains("deliver_file"));
        assert!(tool.description.contains("use write_file"));
        assert!(tool.description.contains("current workbench"));
    }

    #[test]
    fn write_gate_exposes_exactly_whole_file_and_path_tools() {
        // Start from an explicitly write-disabled config so this gate test is
        // independent of the default-on baseline.
        let mut native = crate::settings::ChatNativeToolsConfig::default();
        native.write_file = false;
        native.edit_file = false;
        let defs = list_native_builtin_tool_defs(&native, false, false);
        assert!(defs.is_empty() || !defs.iter().any(|tool| tool.name == "write"));

        native.write_file = true;
        native.edit_file = true;
        let defs = list_native_builtin_tool_defs(&native, false, false);
        let names: Vec<&str> = defs.iter().map(|tool| tool.name.as_str()).collect();
        for expected in ["write", "edit"] {
            assert!(names.contains(&expected), "{expected} should be exposed");
        }
        for removed in [
            "create_dir",
            "delete_path",
            "move_path",
            "copy_path",
            "patch",
            "write_file_chunk",
            "begin_file_write",
            "append_file_write",
            "finish_file_write",
            "abort_file_write",
        ] {
            assert!(!names.contains(&removed), "{removed} must not be exposed");
        }
    }

    #[test]
    fn default_native_config_exposes_file_and_command_tools() {
        // Regression for the "sub-agent stuck on skill_activate" bug: with the
        // agentic default config (all native tools ON), the agent — and its
        // sub-agents, which inherit the same tool table — must actually receive
        // read/ls/grep/find/write/edit/bash. Before the green-light default, only
        // skill tools were exposed, so weak models looped guessing skill names.
        // web_search is intentionally NOT asserted here: it stays gated behind a
        // configured provider key (web_search_configured=false below).
        let native = crate::settings::ChatNativeToolsConfig::default();
        let defs = list_native_builtin_tool_defs(&native, false, false);
        let names: Vec<&str> = defs.iter().map(|tool| tool.name.as_str()).collect();
        for expected in ["read", "grep", "glob", "write", "edit", "bash"] {
            assert!(
                names.contains(&expected),
                "default config must expose `{expected}` (got {names:?})"
            );
        }
    }

    #[test]
    fn mcp_tools_keep_namespaced_openai_names() {
        let server = ChatMcpServer {
            id: "server.one".to_string(),
            name: "Server One".to_string(),
            enabled: true,
            transport: "stdio".to_string(),
            url: String::new(),
            command: "demo".to_string(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        };
        let tool = tool_definition_from_mcp(
            &server,
            McpTool {
                name: "search.web".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({ "type": "object" }),
                output_schema: None,
                annotations: None,
            },
        );

        assert_eq!(tool.openai_tool_name(), "mcp__server_one__search_web");
        assert_ne!(tool.openai_tool_name(), tool.name);
    }

    #[test]
    fn mcp_tool_definition_preserves_metadata_and_read_only_hint() {
        let server = ChatMcpServer {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            enabled: true,
            transport: "stdio".to_string(),
            url: String::new(),
            command: "demo".to_string(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        };
        let annotations = serde_json::json!({ "readOnlyHint": true });
        let output_schema = serde_json::json!({
            "type": "object",
            "properties": { "items": { "type": "array" } }
        });

        let tool = tool_definition_from_mcp(
            &server,
            McpTool {
                name: "search".to_string(),
                description: "Search".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                output_schema: Some(output_schema.clone()),
                annotations: Some(annotations.clone()),
            },
        );

        assert_eq!(tool.annotations.as_ref(), Some(&annotations));
        assert_eq!(tool.output_schema.as_ref(), Some(&output_schema));
        assert_eq!(tool.read_only_hint(), Some(true));
        assert!(tool.is_read_only_tool());
        assert!(tool.sensitive);
    }

    #[test]
    fn mcp_destructive_and_open_world_annotations_are_sensitive() {
        let server = ChatMcpServer {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            enabled: true,
            transport: "stdio".to_string(),
            url: String::new(),
            command: "demo".to_string(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        };

        for annotations in [
            serde_json::json!({ "destructiveHint": true }),
            serde_json::json!({ "openWorldHint": true }),
            serde_json::json!({ "readOnlyHint": false }),
        ] {
            let tool = tool_definition_from_mcp(
                &server,
                McpTool {
                    name: "remote_action".to_string(),
                    description: "Remote action".to_string(),
                    input_schema: serde_json::json!({ "type": "object" }),
                    output_schema: None,
                    annotations: Some(annotations),
                },
            );
            assert!(tool.sensitive);
        }
    }

    #[test]
    fn mcp_open_world_annotation_overrides_read_only_hint() {
        let server = ChatMcpServer {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            enabled: true,
            transport: "stdio".to_string(),
            url: String::new(),
            command: "demo".to_string(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        };

        let tool = tool_definition_from_mcp(
            &server,
            McpTool {
                name: "remote_search".to_string(),
                description: "Remote search".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                output_schema: None,
                annotations: Some(serde_json::json!({
                    "readOnlyHint": true,
                    "openWorldHint": true
                })),
            },
        );

        assert!(tool.sensitive);
        assert!(!tool.is_read_only_tool());
    }

    #[test]
    fn reserved_wire_alias_maps_web_search_both_ways() {
        // Cursor 系上游把 web_search 当内部保留工具吞掉（实测空响应）——wire 层全局改名。
        assert_eq!(apply_reserved_wire_alias("web_search"), "search_web");
        assert_eq!(apply_reserved_wire_alias("web_fetch"), "web_fetch");
        assert_eq!(resolve_reserved_wire_alias("search_web"), "web_search");
        assert_eq!(resolve_reserved_wire_alias("read"), "read");
    }

    #[test]
    fn native_web_search_tool_declares_alias_on_the_wire() {
        let tool = ChatToolDefinition {
            id: "native__web_search".to_string(),
            name: "web_search".to_string(),
            description: "Search the web".to_string(),
            source: "native".to_string(),
            server_id: None,
            server_name: Some("Kivio".to_string()),
            input_schema: serde_json::json!({ "type": "object" }),
            sensitive: false,
            annotations: None,
            output_schema: None,
        };
        assert_eq!(tool.openai_tool_name(), "search_web");
        let wire = tool.to_openai_tool();
        assert_eq!(wire["function"]["name"], "search_web");
        // MCP 工具不受别名影响（按 id 命名）。
        let mcp_tool = ChatToolDefinition {
            source: "mcp".to_string(),
            id: "mcp__srv__web_search".to_string(),
            ..tool
        };
        assert_eq!(mcp_tool.openai_tool_name(), "mcp__srv__web_search");
    }

    #[test]
    fn canonical_tool_name_maps_legacy_names() {
        // 旧名 → 现名（移除/合并/改名后仍可路由）。
        assert_eq!(canonical_tool_name("ls"), "read");
        assert_eq!(canonical_tool_name("find"), "glob");
        assert_eq!(canonical_tool_name("list_background"), "bash_output");
        assert_eq!(canonical_tool_name("todo_update"), "todo_write");
        // 现名与未知名原样返回。
        assert_eq!(canonical_tool_name("read"), "read");
        assert_eq!(canonical_tool_name("glob"), "glob");
        assert_eq!(canonical_tool_name("bash"), "bash");
    }

    #[test]
    fn sanitize_keeps_short_names_and_hash_suffixes_long_ones() {
        assert_eq!(sanitize_openai_tool_name("search.web"), "search_web");
        assert_eq!(sanitize_openai_tool_name("read_file"), "read_file");
        let long = "a".repeat(80);
        let wire = sanitize_openai_tool_name(&long);
        assert_eq!(wire.chars().count(), MAX_WIRE_TOOL_NAME);
        assert!(wire.contains('_'), "{wire}");
        assert_ne!(
            sanitize_openai_tool_name(&format!("{}x", "a".repeat(79))),
            wire,
            "distinct inputs must not collapse after the 64-char cap"
        );
    }

    #[test]
    fn uuid_mcp_server_tools_keep_distinct_wire_names() {
        // Issue #33: `mcp__<uuid>__zai-mcp-server-ui_*` all left-truncated to
        // the same `...-ui` and Gemini dropped the duplicates.
        let server = ChatMcpServer {
            id: "mcp-d1ad400e-e2c7-4f34-9cd0-4e4cf1afa728".to_string(),
            name: "zhipu".to_string(),
            enabled: true,
            transport: "stdio".to_string(),
            url: String::new(),
            command: "demo".to_string(),
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            headers: std::collections::HashMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            connector_id: None,
            auth: None,
        };
        let names = [
            "zai-mcp-server-ui_to_artifact",
            "zai-mcp-server-ui_diff_check",
            "zai-mcp-server-analyze_data_visualization",
            "zai-mcp-server-analyze_image",
            "zai-mcp-server-analyze_video",
        ];
        let mut wires = std::collections::HashSet::new();
        for name in names {
            let tool = tool_definition_from_mcp(
                &server,
                McpTool {
                    name: name.to_string(),
                    description: String::new(),
                    input_schema: serde_json::json!({ "type": "object" }),
                    output_schema: None,
                    annotations: None,
                },
            );
            let wire = tool.openai_tool_name();
            assert!(
                wire.chars().count() <= MAX_WIRE_TOOL_NAME,
                "{wire} exceeds 64"
            );
            assert!(
                wire.contains("zai-mcp-server"),
                "tool suffix should stay visible: {wire}"
            );
            assert!(wires.insert(wire.clone()), "collided: {wire}");
        }
        assert_eq!(wires.len(), names.len());
    }
}
