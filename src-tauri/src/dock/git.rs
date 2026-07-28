//! Dock Git 面板命令：status / diff / log / branches / stage / commit 等。
//! 直接 shell 调用 git 二进制（不引 git2），思路参考 LiveAgent
//! `commands/workspace/git.rs`，精简为 kivio 的 `Result<T, String>` 模型。

use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use wait_timeout::ChildExt;

const GIT_DIFF_MAX_BYTES: usize = 512 * 1024;
const GIT_UNTRACKED_FILE_MAX_BYTES: u64 = 128 * 1024;
const GIT_COMMAND_TIMEOUT_SECS: u64 = 60;
/// index.lock 类瞬时冲突（并行 git 进程）重试：3 次、间隔 160ms。
const GIT_TRANSIENT_RETRY_ATTEMPTS: usize = 3;
const GIT_TRANSIENT_RETRY_DELAY_MS: u64 = 160;
const GIT_LOG_DEFAULT_LIMIT: usize = 50;
const GIT_LOG_MAX_LIMIT: usize = 1000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusEntry {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub index_status: String,
    pub worktree_status: String,
    pub kind: String,
    pub staged: bool,
    pub conflicted: bool,
    pub untracked: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoState {
    pub repo_root: String,
    pub head: String,
    pub upstream: String,
    pub ahead: i32,
    pub behind: i32,
    pub stash_count: i32,
    pub entries: Vec<GitStatusEntry>,
    /// "ready" | "not_repo" | "error"
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffResponse {
    pub base_ref: String,
    pub head_ref: String,
    pub mode: String,
    pub files: Vec<String>,
    pub patch: String,
    pub stat: String,
    pub truncated: bool,
    pub binary_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitItem {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author_name: String,
    pub author_date: String,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLogResponse {
    pub commits: Vec<GitCommitItem>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchItem {
    pub name: String,
    pub current: bool,
    pub upstream: String,
    pub ahead: i32,
    pub behind: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchesResponse {
    pub branches: Vec<GitBranchItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitOperationResponse {
    pub ok: bool,
    pub state: GitRepoState,
    pub stdout: String,
    pub stderr: String,
    pub message: String,
}

struct GitOutput {
    stdout: String,
    stderr: String,
}

// ---- 进程执行 ----

/// 进程组处理复用 kivio `native_tools/shell.rs` 的做法：Unix setsid 让 git
/// 自成进程组，超时杀整组；Windows 起新进程组 + 隐藏控制台。
fn configure_git_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
}

fn kill_git_process_tree(pid: u32, child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        // setsid 后 -pid 命中整个进程组（git 可能再 fork 出 pager/editor 等子进程）。
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_git(workdir: &str, args: &[&str]) -> Result<Output, String> {
    let mut command = Command::new("git");
    configure_git_command(&mut command);
    command
        .args(args)
        .current_dir(workdir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        // 固定消息 locale：transient-lock 重试等逻辑匹配英文文本。
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("git 执行失败：{e}"))?;
    // stdout/stderr 各起一个读取线程：大 diff 会写满 pipe buffer 阻塞子进程，
    // 若等 wait 之后再读会死锁。
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let stdout_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });
    let pid = child.id();
    let Some(status) = child
        .wait_timeout(Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS))
        .map_err(|e| format!("等待 git 命令失败：{e}"))?
    else {
        kill_git_process_tree(pid, &mut child);
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(format!(
            "git 命令超时（{GIT_COMMAND_TIMEOUT_SECS} 秒）：git {}",
            args.join(" ")
        ));
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn trim_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn is_transient_git_lock_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("another git process")
        || lower.contains("index.lock")
        || lower.contains("cannot lock ref")
        || lower.contains("could not lock")
        || (lower.contains("unable to create") && lower.contains(".lock"))
        || lower.contains("failed to lock")
}

fn git_success(workdir: &str, args: &[&str]) -> Result<GitOutput, String> {
    let mut last_error = String::new();
    for attempt in 0..GIT_TRANSIENT_RETRY_ATTEMPTS {
        let output = run_git(workdir, args)?;
        let stdout = trim_output(&output.stdout);
        let stderr = trim_output(&output.stderr);
        if output.status.success() {
            return Ok(GitOutput { stdout, stderr });
        }
        let message = if stderr.is_empty() { stdout } else { stderr };
        if attempt + 1 < GIT_TRANSIENT_RETRY_ATTEMPTS && is_transient_git_lock_error(&message) {
            last_error = message;
            thread::sleep(Duration::from_millis(GIT_TRANSIENT_RETRY_DELAY_MS));
            continue;
        }
        return Err(message);
    }
    Err(last_error)
}

// ---- 仓库定位与状态 ----

fn discover_repo(workdir: &str) -> Result<Option<String>, String> {
    let trimmed = workdir.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let output = run_git(trimmed, &["rev-parse", "--show-toplevel", "--is-inside-work-tree"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let root = lines.next().unwrap_or("").trim().to_string();
    let inside = lines.next().unwrap_or("").trim();
    if root.is_empty() || inside != "true" {
        return Ok(None);
    }
    Ok(Some(root))
}

fn not_repo_state(_workdir: &str) -> GitRepoState {
    GitRepoState {
        repo_root: String::new(),
        head: String::new(),
        upstream: String::new(),
        ahead: 0,
        behind: 0,
        stash_count: 0,
        entries: Vec::new(),
        status: "not_repo".to_string(),
        error: None,
    }
}

fn parse_branch_ab(value: &str) -> (i32, i32) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in value.split_whitespace() {
        if let Some(raw) = part.strip_prefix('+') {
            ahead = raw.parse::<i32>().unwrap_or(0);
        } else if let Some(raw) = part.strip_prefix('-') {
            behind = raw.parse::<i32>().unwrap_or(0);
        }
    }
    (ahead, behind)
}

fn status_entry(
    path: String,
    old_path: Option<String>,
    index: char,
    worktree: char,
    kind: &str,
) -> GitStatusEntry {
    let conflicted = kind == "conflict" || index == 'U' || worktree == 'U';
    let untracked = kind == "untracked";
    let staged = !untracked && !conflicted && index != '.';
    GitStatusEntry {
        path,
        old_path,
        index_status: index.to_string(),
        worktree_status: worktree.to_string(),
        kind: kind.to_string(),
        staged,
        conflicted,
        untracked,
    }
}

fn parse_status_porcelain_v2(raw: &[u8]) -> (String, String, i32, i32, i32, Vec<GitStatusEntry>) {
    let mut head = String::new();
    let mut upstream = String::new();
    let mut ahead = 0;
    let mut behind = 0;
    let mut stash_count = 0;
    let mut entries = Vec::new();
    let records: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    let mut index = 0;
    while index < records.len() {
        let record = records[index].trim_end_matches('\n');
        if let Some(value) = record.strip_prefix("# branch.head ") {
            head = value.trim().to_string();
        } else if let Some(value) = record.strip_prefix("# branch.upstream ") {
            upstream = value.trim().to_string();
        } else if let Some(value) = record.strip_prefix("# branch.ab ") {
            (ahead, behind) = parse_branch_ab(value);
        } else if let Some(value) = record.strip_prefix("# stash ") {
            stash_count = value.trim().parse::<i32>().unwrap_or(0);
        } else if let Some(rest) = record.strip_prefix("1 ") {
            let fields: Vec<&str> = rest.splitn(8, ' ').collect();
            if fields.len() >= 8 {
                let mut chars = fields[0].chars();
                let ix = chars.next().unwrap_or('.');
                let wt = chars.next().unwrap_or('.');
                entries.push(status_entry(
                    fields[7].to_string(),
                    None,
                    ix,
                    wt,
                    "modified",
                ));
            }
        } else if let Some(rest) = record.strip_prefix("2 ") {
            let fields: Vec<&str> = rest.splitn(9, ' ').collect();
            if fields.len() >= 9 {
                let mut chars = fields[0].chars();
                let ix = chars.next().unwrap_or('.');
                let wt = chars.next().unwrap_or('.');
                // rename/copy 的原路径是紧随其后的独立 NUL 记录。
                let old_path = records.get(index + 1).cloned();
                if old_path.is_some() {
                    index += 1;
                }
                entries.push(status_entry(
                    fields[8].to_string(),
                    old_path,
                    ix,
                    wt,
                    "renamed",
                ));
            }
        } else if let Some(rest) = record.strip_prefix("u ") {
            let fields: Vec<&str> = rest.splitn(10, ' ').collect();
            if fields.len() >= 10 {
                let mut chars = fields[0].chars();
                let ix = chars.next().unwrap_or('U');
                let wt = chars.next().unwrap_or('U');
                entries.push(status_entry(
                    fields[9].to_string(),
                    None,
                    ix,
                    wt,
                    "conflict",
                ));
            }
        } else if let Some(path) = record.strip_prefix("? ") {
            entries.push(status_entry(path.to_string(), None, '?', '?', "untracked"));
        }
        index += 1;
    }
    (head, upstream, ahead, behind, stash_count, entries)
}

fn git_status_sync(workdir: &str) -> Result<GitRepoState, String> {
    let workdir = workdir.trim();
    let Some(repo_root) = discover_repo(workdir)? else {
        return Ok(not_repo_state(workdir));
    };
    // --untracked-files=all：否则全未跟踪目录会折叠成单个 `dir/` 条目，
    // 里面的文件进不了 Changes 列表（也没有 diff 可看）。
    let output = run_git(
        &repo_root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--show-stash",
            "--untracked-files=all",
            "-z",
        ],
    )?;
    if !output.status.success() {
        return Ok(GitRepoState {
            repo_root,
            head: String::new(),
            upstream: String::new(),
            ahead: 0,
            behind: 0,
            stash_count: 0,
            entries: Vec::new(),
            status: "error".to_string(),
            error: Some(trim_output(&output.stderr)),
        });
    }
    let (head, upstream, ahead, behind, stash_count, entries) =
        parse_status_porcelain_v2(&output.stdout);
    Ok(GitRepoState {
        repo_root,
        head,
        upstream,
        ahead,
        behind,
        stash_count,
        entries,
        status: "ready".to_string(),
        error: None,
    })
}

fn ensure_ready_state(workdir: &str) -> Result<GitRepoState, String> {
    let state = git_status_sync(workdir)?;
    if state.status == "ready" {
        Ok(state)
    } else {
        Err(state
            .error
            .unwrap_or_else(|| "当前目录不是 Git 仓库。".to_string()))
    }
}

fn ref_exists(repo_root: &str, reference: &str) -> bool {
    run_git(repo_root, &["rev-parse", "--verify", "--quiet", reference])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn validate_repo_relative_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim().replace('\\', "/");
    if trimmed.is_empty() {
        return Err("Git 文件路径不能为空。".to_string());
    }
    let path = Path::new(&trimmed);
    if path.is_absolute() {
        return Err("Git 文件路径不能是绝对路径。".to_string());
    }
    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err("Git 文件路径不能包含 .. 或根路径。".to_string());
        }
    }
    Ok(trimmed)
}

// ---- diff ----

fn split_stat_and_patch(output: &str) -> (String, String) {
    // --stat 与 --patch 同给时，stat 永远排在 patch 之前（与参数顺序无关）。
    let marker = "\ndiff --git ";
    if let Some(index) = output.find(marker) {
        let stat = output[..index].trim().to_string();
        let patch = output[index + 1..].to_string();
        (stat, patch)
    } else if output.starts_with("diff --git ") {
        (String::new(), output.to_string())
    } else {
        (output.trim().to_string(), String::new())
    }
}

fn truncate_patch(value: String) -> (String, bool) {
    if value.len() <= GIT_DIFF_MAX_BYTES {
        return (value, false);
    }
    let mut end = GIT_DIFF_MAX_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

/// 为未跟踪文件合成 unified patch（git diff 不含 untracked 内容）。
/// 超 128KB / 二进制 / 非常规文件返回 None，调用方记入 binaryFiles。
fn build_untracked_file_patch(repo_root: &str, path: &str) -> Result<Option<String>, String> {
    let clean_path = validate_repo_relative_path(path)?;
    let repo_root_path =
        fs::canonicalize(repo_root).map_err(|e| format!("Git 仓库路径不可访问：{e}"))?;
    let absolute_path = fs::canonicalize(Path::new(repo_root).join(&clean_path))
        .map_err(|e| format!("无法读取未跟踪文件 {clean_path}：{e}"))?;
    if !absolute_path.starts_with(&repo_root_path) {
        return Err("Git 文件路径必须位于当前仓库内。".to_string());
    }
    let metadata = fs::metadata(&absolute_path)
        .map_err(|e| format!("无法读取未跟踪文件 {clean_path}：{e}"))?;
    if !metadata.is_file() || metadata.len() > GIT_UNTRACKED_FILE_MAX_BYTES {
        return Ok(None);
    }
    let bytes = fs::read(&absolute_path).map_err(|e| format!("无法读取未跟踪文件 {clean_path}：{e}"))?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    let content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };
    let added_line_count = if content.is_empty() {
        0
    } else {
        content.lines().count().max(1)
    };
    let mut patch = format!(
        "diff --git a/{clean_path} b/{clean_path}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{clean_path}\n@@ -0,0 +1,{added_line_count} @@\n"
    );
    for line in content.split_inclusive('\n') {
        patch.push('+');
        patch.push_str(line.trim_end_matches('\n'));
        patch.push('\n');
    }
    if !content.is_empty() && !content.ends_with('\n') {
        patch.push_str("\\ No newline at end of file\n");
    }
    Ok(Some(patch))
}

fn append_untracked_file_patches(
    repo_root: &str,
    entries: &[GitStatusEntry],
    path_filter: Option<&str>,
    patch: &mut String,
    binary_files: &mut Vec<String>,
) -> Result<(), String> {
    for entry in entries.iter().filter(|entry| entry.untracked) {
        if path_filter.is_some_and(|path| path != entry.path) {
            continue;
        }
        match build_untracked_file_patch(repo_root, &entry.path)? {
            Some(untracked_patch) => {
                if !patch.trim().is_empty() {
                    patch.push('\n');
                }
                patch.push_str(&untracked_patch);
            }
            None => binary_files.push(entry.path.clone()),
        }
    }
    Ok(())
}

/// branch 模式基线：upstream 优先，否则探测 origin/HEAD、origin/main、origin/master。
fn resolve_review_base(state: &GitRepoState) -> String {
    if !state.upstream.trim().is_empty() {
        return state.upstream.clone();
    }
    for candidate in ["origin/HEAD", "origin/main", "origin/master"] {
        if ref_exists(&state.repo_root, candidate) {
            return candidate.to_string();
        }
    }
    String::new()
}

fn git_diff_sync(workdir: &str, mode: Option<&str>, path: Option<&str>) -> Result<GitDiffResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let mode = mode.unwrap_or("branch").to_string();
    if mode != "branch" && mode != "working_tree" {
        return Err(format!("mode 只支持 branch/working_tree，收到：{mode}"));
    }
    let clean_path = path.map(validate_repo_relative_path).transpose()?;
    let files: Vec<String> = state.entries.iter().map(|entry| entry.path.clone()).collect();

    // 仓库还没有任何提交（unborn HEAD）：diff HEAD 必然失败，退化为全部文件
    // 的新增 patch（等价于初始提交的 working_tree 视图）。
    if mode == "working_tree" && !ref_exists(&state.repo_root, "HEAD") {
        let mut patch = String::new();
        let mut binary_files = Vec::new();
        for entry in &state.entries {
            if clean_path.as_deref().is_some_and(|p| p != entry.path) {
                continue;
            }
            match build_untracked_file_patch(&state.repo_root, &entry.path)? {
                Some(initial_patch) => {
                    if !patch.trim().is_empty() {
                        patch.push('\n');
                    }
                    patch.push_str(&initial_patch);
                }
                None => binary_files.push(entry.path.clone()),
            }
        }
        let (patch, truncated) = truncate_patch(patch);
        return Ok(GitDiffResponse {
            base_ref: "ROOT".to_string(),
            head_ref: "WORKTREE".to_string(),
            mode,
            files,
            patch,
            stat: String::new(),
            truncated,
            binary_files,
        });
    }

    let mut base_ref = String::new();
    let mut head_ref = "HEAD".to_string();
    let mut args: Vec<String> = vec!["diff".to_string(), "--patch".to_string(), "--stat".to_string()];
    if mode == "working_tree" {
        args.push("HEAD".to_string());
    } else {
        base_ref = resolve_review_base(&state);
        if base_ref.is_empty() {
            return Err("找不到可用于审查的基线分支。请先设置 upstream 或 fetch 主分支。".to_string());
        }
        args.push(format!("{base_ref}...HEAD"));
    }
    if let Some(path) = clean_path.as_deref() {
        args.push("--".to_string());
        args.push(path.to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = git_success(&state.repo_root, &arg_refs)?;
    let (stat, mut patch) = split_stat_and_patch(&output.stdout);
    let mut binary_files = Vec::new();
    if mode == "working_tree" {
        append_untracked_file_patches(
            &state.repo_root,
            &state.entries,
            clean_path.as_deref(),
            &mut patch,
            &mut binary_files,
        )?;
        base_ref = "HEAD".to_string();
        head_ref = "WORKTREE".to_string();
    }
    let (patch, truncated) = truncate_patch(patch);
    Ok(GitDiffResponse {
        base_ref,
        head_ref,
        mode,
        files,
        patch,
        stat,
        truncated,
        binary_files,
    })
}

#[tauri::command]
pub async fn dock_git_diff(
    workdir: String,
    mode: Option<String>,
    path: Option<String>,
) -> Result<GitDiffResponse, String> {
    tauri::async_runtime::spawn_blocking(move || git_diff_sync(&workdir, mode.as_deref(), path.as_deref()))
        .await
        .map_err(|e| format!("dock_git_diff join: {e}"))?
}

fn validate_commit(repo_root: &str, value: &str) -> Result<String, String> {
    let commit = value.trim();
    if commit.is_empty() {
        return Err("提交引用不能为空。".to_string());
    }
    git_success(
        repo_root,
        &["rev-parse", "--verify", "--quiet", &format!("{commit}^{{commit}}")],
    )?;
    Ok(commit.to_string())
}

fn git_commit_diff_sync(workdir: &str, commit: &str, path: Option<&str>) -> Result<GitDiffResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let commit = validate_commit(&state.repo_root, commit)?;
    let clean_path = path.map(validate_repo_relative_path).transpose()?;
    let parent_output = git_success(&state.repo_root, &["show", "-s", "--format=%P", &commit])?;
    let first_parent = parent_output
        .stdout
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    // 根提交没有父提交可 diff，用 git show 自身的 patch 输出。
    let mut args: Vec<String> = if first_parent.is_empty() {
        vec![
            "show".to_string(),
            "--format=".to_string(),
            "--patch".to_string(),
            "--stat".to_string(),
            "--find-renames".to_string(),
            commit.clone(),
        ]
    } else {
        vec![
            "diff".to_string(),
            "--patch".to_string(),
            "--stat".to_string(),
            "--find-renames".to_string(),
            first_parent.clone(),
            commit.clone(),
        ]
    };
    if let Some(path) = clean_path.as_deref() {
        args.push("--".to_string());
        args.push(path.to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = git_success(&state.repo_root, &arg_refs)?;
    let (stat, patch) = split_stat_and_patch(&output.stdout);
    let (patch, truncated) = truncate_patch(patch);
    Ok(GitDiffResponse {
        base_ref: if first_parent.is_empty() {
            "ROOT".to_string()
        } else {
            first_parent
        },
        head_ref: commit,
        mode: "commit".to_string(),
        files: clean_path.into_iter().collect(),
        patch,
        stat,
        truncated,
        binary_files: Vec::new(),
    })
}

#[tauri::command]
pub async fn dock_git_commit_diff(
    workdir: String,
    commit: String,
    path: Option<String>,
) -> Result<GitDiffResponse, String> {
    tauri::async_runtime::spawn_blocking(move || git_commit_diff_sync(&workdir, &commit, path.as_deref()))
        .await
        .map_err(|e| format!("dock_git_commit_diff join: {e}"))?
}

// ---- log ----

fn parse_git_refs(raw: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for part in raw.split(',') {
        let value = part.trim();
        if value.is_empty() || value == "HEAD" {
            continue;
        }
        let label = value.strip_prefix("tag: ").unwrap_or(value).to_string();
        if !refs.contains(&label) {
            refs.push(label);
        }
    }
    refs
}

fn parse_git_log(raw: &str) -> Vec<GitCommitItem> {
    raw.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_matches(|ch: char| ch == '\0' || ch.is_whitespace());
            if record.is_empty() {
                return None;
            }
            let fields: Vec<&str> = record.split('\x1f').collect();
            if fields.len() < 6 {
                return None;
            }
            let sha = fields[0].trim().to_string();
            if sha.is_empty() {
                return None;
            }
            Some(GitCommitItem {
                sha,
                short_sha: fields[1].trim().to_string(),
                refs: parse_git_refs(fields[2]),
                author_name: fields[3].trim().to_string(),
                author_date: fields[4].trim().to_string(),
                subject: fields[5].trim().to_string(),
            })
        })
        .collect()
}

fn git_log_sync(workdir: &str, limit: Option<usize>, skip: Option<usize>) -> Result<GitLogResponse, String> {
    let state = ensure_ready_state(workdir)?;
    if !ref_exists(&state.repo_root, "HEAD") {
        return Ok(GitLogResponse {
            commits: Vec::new(),
            has_more: false,
        });
    }
    let limit = limit.unwrap_or(GIT_LOG_DEFAULT_LIMIT).clamp(1, GIT_LOG_MAX_LIMIT);
    let skip = skip.unwrap_or(0);
    // 多取一条判断 hasMore，避免再来一次 rev-list 计数。
    let mut args = vec![
        "log".to_string(),
        "--date=iso-strict".to_string(),
        "--decorate=short".to_string(),
        "--pretty=format:%x1e%H%x1f%h%x1f%D%x1f%an%x1f%aI%x1f%s".to_string(),
        "--max-count".to_string(),
        (limit + 1).to_string(),
    ];
    if skip > 0 {
        args.push(format!("--skip={skip}"));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = git_success(&state.repo_root, &arg_refs)?;
    let mut commits = parse_git_log(&output.stdout);
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    Ok(GitLogResponse { commits, has_more })
}

#[tauri::command]
pub async fn dock_git_log(
    workdir: String,
    limit: Option<usize>,
    skip: Option<usize>,
) -> Result<GitLogResponse, String> {
    tauri::async_runtime::spawn_blocking(move || git_log_sync(&workdir, limit, skip))
        .await
        .map_err(|e| format!("dock_git_log join: {e}"))?
}

// ---- branches ----

/// 解析 for-each-ref 的 %(upstream:track)：`[ahead 1, behind 2]` / `[gone]` / 空。
fn parse_upstream_track(value: &str) -> (i32, i32) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in value.trim_matches(|ch| ch == '[' || ch == ']').split(',') {
        let mut tokens = part.trim().split_whitespace();
        match (tokens.next(), tokens.next()) {
            (Some("ahead"), Some(n)) => ahead = n.parse().unwrap_or(0),
            (Some("behind"), Some(n)) => behind = n.parse().unwrap_or(0),
            _ => {}
        }
    }
    (ahead, behind)
}

fn git_branches_sync(workdir: &str) -> Result<GitBranchesResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let output = git_success(
        &state.repo_root,
        &[
            "for-each-ref",
            "--format=%(refname:short)%00%(upstream:short)%00%(HEAD)%00%(upstream:track)",
            "refs/heads",
        ],
    )?;
    let mut branches = Vec::new();
    for line in output.stdout.lines() {
        let parts: Vec<&str> = line.split('\0').collect();
        if parts.len() < 4 {
            continue;
        }
        let name = parts[0].trim();
        if name.is_empty() {
            continue;
        }
        let (ahead, behind) = parse_upstream_track(parts[3]);
        branches.push(GitBranchItem {
            name: name.to_string(),
            current: parts[2].trim() == "*" || name == state.head,
            upstream: parts[1].trim().to_string(),
            ahead,
            behind,
        });
    }
    Ok(GitBranchesResponse { branches })
}

#[tauri::command]
pub async fn dock_git_branches(workdir: String) -> Result<GitBranchesResponse, String> {
    tauri::async_runtime::spawn_blocking(move || git_branches_sync(&workdir))
        .await
        .map_err(|e| format!("dock_git_branches join: {e}"))?
}

// ---- 变更操作 ----

/// 操作结束后重跑 status，把最新仓库状态一并返回给前端。
fn operation_response(
    workdir: &str,
    result: Result<GitOutput, String>,
    success_message: &str,
) -> Result<GitOperationResponse, String> {
    let state = git_status_sync(workdir)?;
    match result {
        Ok(output) => Ok(GitOperationResponse {
            ok: true,
            state,
            stdout: output.stdout,
            stderr: output.stderr,
            message: success_message.to_string(),
        }),
        Err(error) => Ok(GitOperationResponse {
            ok: false,
            state,
            stdout: String::new(),
            stderr: error.clone(),
            message: error,
        }),
    }
}

fn validate_branch_name(repo_root: &str, branch: &str) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("分支名不能为空。".to_string());
    }
    if branch.chars().any(char::is_whitespace) {
        return Err("分支名不能包含空白字符。".to_string());
    }
    git_success(repo_root, &["check-ref-format", "--branch", branch])?;
    Ok(branch.to_string())
}

macro_rules! dock_git_mutation {
    ($name:ident, $sync:ident, ($($arg:ident : $ty:ty),*)) => {
        #[tauri::command]
        pub async fn $name(workdir: String, $($arg: $ty),*) -> Result<GitOperationResponse, String> {
            tauri::async_runtime::spawn_blocking(move || $sync(&workdir, $($arg),*))
                .await
                .map_err(|e| format!("{} join: {e}", stringify!($name)))?
        }
    };
}

fn git_stage_sync(workdir: &str, path: String) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let path = validate_repo_relative_path(&path)?;
    operation_response(
        workdir,
        git_success(&state.repo_root, &["add", "--", path.as_str()]),
        "文件已暂存。",
    )
}

dock_git_mutation!(dock_git_stage, git_stage_sync, (path: String));

fn git_stage_all_sync(workdir: &str) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    operation_response(
        workdir,
        git_success(&state.repo_root, &["add", "-A", "--"]),
        "所有改动已暂存。",
    )
}

dock_git_mutation!(dock_git_stage_all, git_stage_all_sync, ());

fn git_unstage_sync(workdir: &str, path: String) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let path = validate_repo_relative_path(&path)?;
    // 仓库还没有任何提交时 restore --staged 没有 HEAD 可恢复，用 rm --cached。
    let staged_without_head = !ref_exists(&state.repo_root, "HEAD")
        && state
            .entries
            .iter()
            .any(|entry| entry.path == path && !entry.untracked && entry.index_status != ".");
    let result = if staged_without_head {
        git_success(&state.repo_root, &["rm", "--cached", "--", path.as_str()])
    } else {
        git_success(&state.repo_root, &["restore", "--staged", "--", path.as_str()])
    };
    operation_response(workdir, result, "文件已取消暂存。")
}

dock_git_mutation!(dock_git_unstage, git_unstage_sync, (path: String));

fn git_unstage_all_sync(workdir: &str) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let result = if !ref_exists(&state.repo_root, "HEAD") {
        if state.entries.iter().any(|entry| entry.staged) {
            git_success(&state.repo_root, &["rm", "--cached", "-r", "--", "."])
        } else {
            Ok(GitOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    } else {
        git_success(&state.repo_root, &["restore", "--staged", "--", "."])
    };
    operation_response(workdir, result, "所有改动已取消暂存。")
}

dock_git_mutation!(dock_git_unstage_all, git_unstage_all_sync, ());

fn git_discard_sync(
    workdir: &str,
    path: String,
    old_path: Option<String>,
) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let path = validate_repo_relative_path(&path)?;
    let old_path = old_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(validate_repo_relative_path)
        .transpose()?;
    let is_untracked = state
        .entries
        .iter()
        .any(|entry| entry.path == path && entry.untracked);
    let staged_without_head = !ref_exists(&state.repo_root, "HEAD")
        && state
            .entries
            .iter()
            .any(|entry| entry.path == path && !entry.untracked && entry.index_status != ".");
    let result = if is_untracked {
        // 未跟踪文件没有可恢复内容，discard 等于删除。
        git_success(&state.repo_root, &["clean", "-fd", "--", path.as_str()])
    } else if staged_without_head {
        git_success(&state.repo_root, &["rm", "-f", "--", path.as_str()])
    } else {
        // restore --staged --worktree：暂存区与工作区一起回滚（覆盖 checkout --
        // 只回工作区的语义，符合 review 面板"放弃改动"的直觉）。
        let mut args = vec!["restore", "--staged", "--worktree", "--", path.as_str()];
        if let Some(old_path) = old_path.as_deref() {
            if old_path != path {
                args.push(old_path);
            }
        }
        git_success(&state.repo_root, &args)
    };
    operation_response(workdir, result, "改动已放弃。")
}

dock_git_mutation!(dock_git_discard, git_discard_sync, (path: String, old_path: Option<String>));

fn git_discard_all_sync(workdir: &str) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let result = if !ref_exists(&state.repo_root, "HEAD") {
        let remove_result = if state.entries.iter().any(|entry| entry.staged) {
            git_success(&state.repo_root, &["rm", "-f", "-r", "--", "."])
        } else {
            Ok(GitOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        };
        remove_result.and_then(|remove_output| {
            git_success(&state.repo_root, &["clean", "-fd", "--", "."]).map(|clean_output| GitOutput {
                stdout: [remove_output.stdout, clean_output.stdout]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
                stderr: [remove_output.stderr, clean_output.stderr]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            })
        })
    } else {
        git_success(&state.repo_root, &["restore", "--staged", "--worktree", "--", "."])
            .and_then(|restore_output| {
                git_success(&state.repo_root, &["clean", "-fd", "--", "."]).map(|clean_output| GitOutput {
                    stdout: [restore_output.stdout, clean_output.stdout]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    stderr: [restore_output.stderr, clean_output.stderr]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n"),
                })
            })
    };
    operation_response(workdir, result, "所有改动已放弃。")
}

dock_git_mutation!(dock_git_discard_all, git_discard_all_sync, ());

fn git_commit_sync(workdir: &str, message: String) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("Commit message 不能为空。".to_string());
    }
    if !state.entries.iter().any(|entry| entry.staged) {
        return Err("没有已暂存的改动可提交。".to_string());
    }
    git_success(&state.repo_root, &["config", "--get", "user.name"])
        .map_err(|_| "Git user.name 未配置。".to_string())?;
    git_success(&state.repo_root, &["config", "--get", "user.email"])
        .map_err(|_| "Git user.email 未配置。".to_string())?;
    // message 作为独立 argv 传递：Command 不过 shell，无注入问题。
    operation_response(
        workdir,
        git_success(&state.repo_root, &["commit", "-m", message.as_str()]),
        "提交已创建。",
    )
}

dock_git_mutation!(dock_git_commit, git_commit_sync, (message: String));

fn git_switch_branch_sync(workdir: &str, branch: String) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let branch = validate_branch_name(&state.repo_root, &branch)?;
    operation_response(
        workdir,
        git_success(&state.repo_root, &["switch", branch.as_str()]),
        "分支已切换。",
    )
}

dock_git_mutation!(dock_git_switch_branch, git_switch_branch_sync, (branch: String));

fn git_create_branch_sync(
    workdir: &str,
    branch: String,
    start_point: Option<String>,
) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let branch = validate_branch_name(&state.repo_root, &branch)?;
    let start_point = start_point
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            git_success(&state.repo_root, &["rev-parse", "--verify", "--quiet", value])
                .map(|_| value.to_string())
        })
        .transpose()?;
    let mut args = vec!["switch", "-c", branch.as_str()];
    if let Some(start_point) = start_point.as_deref() {
        args.push(start_point);
    }
    operation_response(
        workdir,
        git_success(&state.repo_root, &args),
        "分支已创建并检出。",
    )
}

dock_git_mutation!(dock_git_create_branch, git_create_branch_sync, (branch: String, start_point: Option<String>));

fn git_init_sync(workdir: &str, branch: Option<String>) -> Result<GitOperationResponse, String> {
    // init 是唯一不要求「已是仓库」的操作：目标目录本身必须存在（与 fs 命令同一约定）。
    // git init 幂等（已是仓库时输出 Reinitialized），无需前置检查。
    let root = std::fs::canonicalize(workdir)
        .map_err(|e| format!("工作目录不存在或不可访问：{e}"))?;
    if !root.is_dir() {
        return Err("工作目录不是目录。".to_string());
    }
    let root = root.to_string_lossy().to_string();
    let branch = branch
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(name) = branch.as_deref() {
        if name.starts_with('-')
            || name.contains("..")
            || name.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("分支名不合法。".to_string());
        }
    }
    let mut args: Vec<&str> = vec!["init"];
    if let Some(name) = branch.as_deref() {
        args.push("-b");
        args.push(name);
    }
    operation_response(&root, git_success(&root, &args), "Git 仓库已初始化。")
}

dock_git_mutation!(dock_git_init, git_init_sync, (branch: Option<String>));

// ---- 查询命令（diff 统计） ----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffStatFile {
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffStatResponse {
    pub files_changed: usize,
    pub additions: usize,
    pub deletions: usize,
    /// 逐文件明细：Git 面板改动列表行内徽标用。
    pub files: Vec<GitDiffStatFile>,
}

/// 状态条 diff 徽标：汇总工作区相对 HEAD 的 numstat（staged + unstaged），
/// untracked 文件按行数计为新增（与 patch 合成同一 128KB / 文本约束，二进制不计行）。
fn git_diff_stat_sync(workdir: &str) -> Result<GitDiffStatResponse, String> {
    let state = git_status_sync(workdir)?;
    if state.status != "ready" {
        return Ok(GitDiffStatResponse {
            files_changed: 0,
            additions: 0,
            deletions: 0,
            files: Vec::new(),
        });
    }
    let repo = state.repo_root.as_str();
    // 有 HEAD：`diff HEAD` 同时覆盖 staged 与 unstaged；unborn HEAD 时 tracked 改动
    // 只可能是已暂存的新文件，`diff --cached` 自动与空树比较。
    let numstat_args: &[&str] = if ref_exists(repo, "HEAD") {
        &["diff", "--numstat", "HEAD", "--"]
    } else {
        &["diff", "--numstat", "--cached", "--"]
    };
    let output = git_success(repo, numstat_args)?;
    let mut files: Vec<GitDiffStatFile> = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for line in output.stdout.lines() {
        let mut cols = line.split('\t');
        let (Some(adds), Some(dels), Some(path)) = (cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        // 二进制行是 "-\t-"，numstat 不计入行数但算一个改动文件。
        let (file_adds, file_dels) = match (adds.parse::<usize>(), dels.parse::<usize>()) {
            (Ok(a), Ok(d)) => (a, d),
            _ => (0, 0),
        };
        additions += file_adds;
        deletions += file_dels;
        files.push(GitDiffStatFile {
            path: path.to_string(),
            additions: file_adds,
            deletions: file_dels,
        });
    }
    for entry in state.entries.iter().filter(|entry| entry.untracked) {
        let file_adds = match build_untracked_file_patch(repo, &entry.path) {
            Ok(Some(patch)) => patch
                .lines()
                .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
                .count(),
            _ => 0,
        };
        additions += file_adds;
        files.push(GitDiffStatFile {
            path: entry.path.clone(),
            additions: file_adds,
            deletions: 0,
        });
    }
    let files_changed = files.len();
    Ok(GitDiffStatResponse {
        files_changed,
        additions,
        deletions,
        files,
    })
}

#[tauri::command]
pub async fn dock_git_diff_stat(workdir: String) -> Result<GitDiffStatResponse, String> {
    tauri::async_runtime::spawn_blocking(move || git_diff_stat_sync(&workdir))
        .await
        .map_err(|e| format!("dock_git_diff_stat join: {e}"))?
}

fn git_add_to_gitignore_sync(workdir: &str, path: String) -> Result<GitOperationResponse, String> {
    let state = ensure_ready_state(workdir)?;
    let path = validate_repo_relative_path(&path)?;
    let pattern = format!("/{path}");
    let gitignore_path = Path::new(&state.repo_root).join(".gitignore");
    let mut content = match fs::read_to_string(&gitignore_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("读取 .gitignore 失败：{e}")),
    };
    let already_present = content
        .lines()
        .any(|line| line.trim() == path || line.trim() == pattern);
    let result = if already_present {
        Ok(GitOutput {
            stdout: String::new(),
            stderr: String::new(),
        })
    } else {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&pattern);
        content.push('\n');
        fs::write(&gitignore_path, content)
            .map(|_| GitOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
            .map_err(|e| format!("写入 .gitignore 失败：{e}"))
    };
    operation_response(
        workdir,
        result,
        if already_present {
            "路径已存在于 .gitignore。"
        } else {
            "路径已添加到 .gitignore。"
        },
    )
}

dock_git_mutation!(dock_git_add_to_gitignore, git_add_to_gitignore_sync, (path: String));

// ---- 查询命令（status） ----

#[tauri::command]
pub async fn dock_git_status(workdir: String) -> Result<GitRepoState, String> {
    tauri::async_runtime::spawn_blocking(move || git_status_sync(&workdir))
        .await
        .map_err(|e| format!("dock_git_status join: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kivio-dock-git-{tag}-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::canonicalize(&dir).expect("canonicalize temp dir")
    }

    #[test]
    fn porcelain_v2_parses_all_entry_kinds() {
        // branch 头 + modified / added / deleted / renamed / conflicted / untracked。
        let mut raw = Vec::new();
        raw.extend_from_slice(b"# branch.head main\0");
        raw.extend_from_slice(b"# branch.upstream origin/main\0");
        raw.extend_from_slice(b"# branch.ab +2 -1\0");
        raw.extend_from_slice(b"# stash 3\0");
        raw.extend_from_slice(b"1 .M N... 100644 100644 100644 abc def modified.txt\0");
        raw.extend_from_slice(b"1 A. N... 000000 100644 100644 000 abc added.txt\0");
        raw.extend_from_slice(b"1 .D N... 100644 000000 000000 abc 000 deleted.txt\0");
        raw.extend_from_slice(b"2 R. N... 100644 100644 100644 abc def R100 new-name.txt\0");
        raw.extend_from_slice(b"old-name.txt\0");
        raw.extend_from_slice(b"u UU N... 100644 100644 100644 100644 abc def ghi conflicted.txt\0");
        raw.extend_from_slice(b"? fresh.txt\0");

        let (head, upstream, ahead, behind, stash, entries) = parse_status_porcelain_v2(&raw);
        assert_eq!(head, "main");
        assert_eq!(upstream, "origin/main");
        assert_eq!((ahead, behind), (2, 1));
        assert_eq!(stash, 3);
        assert_eq!(entries.len(), 6);

        let by_path = |p: &str| entries.iter().find(|e| e.path == p).unwrap_or_else(|| panic!("missing {p}"));
        let modified = by_path("modified.txt");
        assert_eq!(modified.kind, "modified");
        assert!(!modified.staged && !modified.conflicted && !modified.untracked);
        assert_eq!(modified.worktree_status, "M");

        let added = by_path("added.txt");
        assert!(added.staged);
        assert_eq!(added.index_status, "A");

        let deleted = by_path("deleted.txt");
        assert_eq!(deleted.worktree_status, "D");
        assert!(!deleted.staged);

        let renamed = by_path("new-name.txt");
        assert_eq!(renamed.kind, "renamed");
        assert_eq!(renamed.old_path.as_deref(), Some("old-name.txt"));
        assert!(renamed.staged);

        let conflicted = by_path("conflicted.txt");
        assert_eq!(conflicted.kind, "conflict");
        assert!(conflicted.conflicted);
        assert!(!conflicted.staged);

        let untracked = by_path("fresh.txt");
        assert!(untracked.untracked);
        assert!(!untracked.staged);
    }

    #[test]
    fn log_format_parses_records_and_refs() {
        let raw = "\x1eabc123def456\x1fabc123d\x1fHEAD -> main, tag: v1.0, origin/main\x1fAlice\x1f2026-07-27T10:00:00+08:00\x1ffeat: first\n\x1e789xyz\x1f789xyz0\x1f\x1fBob\x1f2026-07-26T09:00:00+08:00\x1fchore: second\n";
        let commits = parse_git_log(raw);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "abc123def456");
        assert_eq!(commits[0].short_sha, "abc123d");
        assert_eq!(commits[0].subject, "feat: first");
        assert_eq!(commits[0].author_name, "Alice");
        assert_eq!(commits[0].author_date, "2026-07-27T10:00:00+08:00");
        assert_eq!(
            commits[0].refs,
            vec!["HEAD -> main".to_string(), "v1.0".to_string(), "origin/main".to_string()]
        );
        assert!(commits[1].refs.is_empty());
    }

    #[test]
    fn upstream_track_parses_ahead_behind() {
        assert_eq!(parse_upstream_track("[ahead 1, behind 2]"), (1, 2));
        assert_eq!(parse_upstream_track("[ahead 3]"), (3, 0));
        assert_eq!(parse_upstream_track("[behind 4]"), (0, 4));
        assert_eq!(parse_upstream_track("[gone]"), (0, 0));
        assert_eq!(parse_upstream_track(""), (0, 0));
    }

    #[test]
    fn untracked_patch_synthesis() {
        let dir = temp_dir("patch");
        fs::write(dir.join("note.txt"), "hello\nworld\n").expect("write note");
        let patch = build_untracked_file_patch(dir.to_str().unwrap(), "note.txt")
            .expect("patch ok")
            .expect("patch present");
        assert!(patch.contains("diff --git a/note.txt b/note.txt"));
        assert!(patch.contains("new file mode 100644"));
        assert!(patch.contains("@@ -0,0 +1,2 @@"));
        assert!(patch.contains("+hello"));
        assert!(patch.contains("+world"));

        // 无结尾换行：追加 "\ No newline at end of file"。
        fs::write(dir.join("noeol.txt"), "tail").expect("write noeol");
        let patch = build_untracked_file_patch(dir.to_str().unwrap(), "noeol.txt")
            .expect("patch ok")
            .expect("patch present");
        assert!(patch.contains("\\ No newline at end of file"));

        // 含 NUL 视为二进制，返回 None。
        fs::write(dir.join("bin.dat"), [0u8, 1, 2]).expect("write bin");
        assert!(build_untracked_file_patch(dir.to_str().unwrap(), "bin.dat")
            .expect("ok")
            .is_none());

        // 路径穿越被拒绝。
        assert!(build_untracked_file_patch(dir.to_str().unwrap(), "../evil.txt").is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn init_creates_repo_in_plain_dir() {
        let dir = temp_dir("init");
        let root = dir.to_str().unwrap().to_string();
        // 空目录 init：ready、无提交、默认分支解析成功。
        let response = git_init_sync(&root, None).expect("init ok");
        assert!(response.ok);
        assert_eq!(response.state.status, "ready");
        assert!(response.state.entries.is_empty());
        // 幂等：已是仓库再 init 不报错。
        let again = git_init_sync(&root, Some("main".to_string())).expect("re-init ok");
        assert!(again.ok);
        // 非法分支名被拒绝。
        assert!(git_init_sync(&root, Some("-bad".to_string())).is_err());
        assert!(git_init_sync(&root, Some("a b".to_string())).is_err());
        // 不存在的目录被拒绝。
        assert!(git_init_sync("/nonexistent/kivio-dock-git-init-dir", None).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diff_stat_sums_tracked_and_untracked() {
        let dir = temp_dir("stat");
        let root = dir.to_str().unwrap().to_string();
        git_init_sync(&root, None).expect("init ok");
        // 基线提交：3 行文件。
        fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").expect("write a");
        git_success(&root, &["add", "-A", "--"]).expect("add");
        git_success(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "init",
            ],
        )
        .expect("commit");
        // 干净工作区：全零。
        let clean = git_diff_stat_sync(&root).expect("stat ok");
        assert_eq!(
            (clean.files_changed, clean.additions, clean.deletions),
            (0, 0, 0)
        );
        // 改 1 行（+1 −1）+ 新增 tracked 之外一个 2 行 untracked 文件（+2）。
        fs::write(dir.join("a.txt"), "one\ntwo\nTHREE\n").expect("edit a");
        fs::write(dir.join("b.txt"), "x\ny\n").expect("write b");
        let dirty = git_diff_stat_sync(&root).expect("stat ok");
        assert_eq!(dirty.files_changed, 2);
        assert_eq!(dirty.additions, 3);
        assert_eq!(dirty.deletions, 1);
        // 逐文件明细：a.txt +1 −1，b.txt +2 −0。
        let a = dirty.files.iter().find(|f| f.path == "a.txt").expect("a.txt stat");
        assert_eq!((a.additions, a.deletions), (1, 1));
        let b = dirty.files.iter().find(|f| f.path == "b.txt").expect("b.txt stat");
        assert_eq!((b.additions, b.deletions), (2, 0));
        fs::remove_dir_all(&dir).ok();
    }
}
