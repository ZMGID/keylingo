//! Workspace 变更监听：对 workdir 做递归文件监听，去抖后向前端发
//! `workspace:activity` 事件，驱动文件树 / Git 面板秒级刷新。
//! 思路参考 LiveAgent `services/workspace_watch/`，精简为单来源（本地 webview）。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::Emitter;

pub const WORKSPACE_ACTIVITY_EVENT: &str = "workspace:activity";

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_STOP_CHECK: Duration = Duration::from_millis(250);
const MAX_CHANGED_PATHS: usize = 64;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceActivityPayload {
    workdir: String,
    revision: u64,
    fs: bool,
    git: bool,
    changed_paths: Vec<String>,
    truncated: bool,
}

#[derive(Default)]
struct WatchInner {
    desired: BTreeSet<String>,
    watchers: HashMap<String, WatcherHandle>,
}

pub struct WorkspaceWatchService {
    app_handle: tauri::AppHandle,
    inner: Mutex<WatchInner>,
    // 每目录单调 revision：放在 WatchInner 外，watcher 重建不归零
    // （前端把 revision 回退视为强制失效）。
    revisions: Mutex<HashMap<String, u64>>,
}

impl WorkspaceWatchService {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self {
            app_handle,
            inner: Mutex::new(WatchInner::default()),
            revisions: Mutex::new(HashMap::new()),
        }
    }

    /// 整体替换目标 watch 集合，diff 出该停的与该启的 watcher。
    pub fn set_desired(self: &Arc<Self>, workdirs: Vec<String>) {
        let desired: BTreeSet<String> = workdirs
            .into_iter()
            .map(|workdir| workdir.trim().to_string())
            .filter(|workdir| !workdir.is_empty())
            .collect();
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.desired = desired.clone();
        // drop handle 即停 watcher：native watcher 断开聚合线程 channel，
        // 轮询兜底线程观察到 stop 标志退出。
        inner
            .watchers
            .retain(|workdir, _| desired.contains(workdir));
        for workdir in &desired {
            if !inner.watchers.contains_key(workdir) {
                let handle = spawn_workdir_watcher(workdir.clone(), Arc::downgrade(self));
                inner.watchers.insert(workdir.clone(), handle);
            }
        }
    }

    fn next_revision(&self, workdir: &str) -> u64 {
        let Ok(mut revisions) = self.revisions.lock() else {
            // 锁中毒返回 0：前端视为 revision 回退 → 强制刷新，fail-safe。
            return 0;
        };
        let counter = revisions.entry(workdir.to_string()).or_insert(0);
        *counter += 1;
        *counter
    }

    fn emit_activity(
        &self,
        workdir: &str,
        fs: bool,
        git: bool,
        changed_paths: Vec<String>,
        truncated: bool,
    ) {
        let payload = WorkspaceActivityPayload {
            workdir: workdir.to_string(),
            revision: self.next_revision(workdir),
            fs,
            git,
            changed_paths,
            truncated,
        };
        if let Err(error) = self.app_handle.emit(WORKSPACE_ACTIVITY_EVENT, payload) {
            eprintln!("workspace watcher: emit activity for {workdir} failed: {error}");
        }
    }
}

/// 前端声明关注的工作目录集合（全量替换语义）。
#[tauri::command]
pub async fn dock_workspace_watch_set(
    state: tauri::State<'_, Arc<WorkspaceWatchService>>,
    workdirs: Vec<String>,
) -> Result<(), String> {
    let service: &Arc<WorkspaceWatchService> = state.inner();
    service.set_desired(workdirs);
    Ok(())
}

// ---- 每目录 watcher ----

/// 保持 watcher 存活的句柄：drop 即停。
struct WatcherHandle {
    _watcher: Option<RecommendedWatcher>,
    stop: Arc<AtomicBool>,
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// workdir 的 git 元数据布局：`.git` 是目录则已在递归监听范围内；`.git`
/// 是文件（linked worktree / submodule 的 `gitdir: <path>` 指针）时，真实
/// gitdir 在 workdir 之外，需要单独挂监听并用于事件路径分类。
struct GitMeta {
    /// 需要额外挂递归监听的外部 gitdir（去重后）。
    watch_roots: Vec<PathBuf>,
    /// 分类前缀（含 canonicalize 变体，FSEvents 可能上报解析后路径），长的在前。
    class_roots: Vec<PathBuf>,
    /// 该 worktree 的 gitdir（轮询兜底采样 HEAD/index 用）；非仓库时为 workdir/.git。
    git_dir: PathBuf,
}

fn resolve_git_meta(workdir: &Path) -> GitMeta {
    let mut watch_roots = Vec::new();
    let mut class_roots = Vec::new();
    let mut git_dir = workdir.join(".git");
    let dot_git = workdir.join(".git");
    if dot_git.is_file() {
        if let Some(resolved) = resolve_gitdir_file(&dot_git) {
            // commondir 指向共享 refs 所在目录（linked worktree），一并监听。
            if let Some(common) = resolve_commondir(&resolved) {
                push_root(&mut watch_roots, &mut class_roots, common);
            }
            git_dir = resolved.clone();
            push_root(&mut watch_roots, &mut class_roots, resolved);
        }
    }
    class_roots.sort_by_key(|root| std::cmp::Reverse(root.as_os_str().len()));
    GitMeta {
        watch_roots,
        class_roots,
        git_dir,
    }
}

fn push_root(watch_roots: &mut Vec<PathBuf>, class_roots: &mut Vec<PathBuf>, root: PathBuf) {
    if !class_roots.contains(&root) {
        class_roots.push(root.clone());
    }
    if let Ok(canonical) = std::fs::canonicalize(&root) {
        if !class_roots.contains(&canonical) {
            class_roots.push(canonical);
        }
    }
    // 已被其他 root 覆盖（嵌套）时不重复挂监听。
    if watch_roots
        .iter()
        .any(|existing| root.starts_with(existing))
    {
        return;
    }
    watch_roots.retain(|existing| !existing.starts_with(&root));
    watch_roots.push(root);
}

/// 解析 `.git` 文件的 `gitdir: <path>` 指针（相对路径锚在文件所在目录）。
fn resolve_gitdir_file(dot_git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(dot_git_file).ok()?;
    let target = content
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    if target.is_empty() {
        return None;
    }
    let raw = Path::new(target);
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        dot_git_file.parent()?.join(raw)
    };
    Some(std::fs::canonicalize(&resolved).unwrap_or(resolved))
}

/// 解析 gitdir 的 `commondir` 指针（相对路径锚在 gitdir）。
fn resolve_commondir(git_dir: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
    let target = content.trim();
    if target.is_empty() {
        return None;
    }
    let raw = Path::new(target);
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        git_dir.join(raw)
    };
    Some(std::fs::canonicalize(&resolved).unwrap_or(resolved))
}

fn spawn_workdir_watcher(workdir: String, service: Weak<WorkspaceWatchService>) -> WatcherHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
    let meta = resolve_git_meta(Path::new(&workdir));

    let watcher = RecommendedWatcher::new(tx, Config::default()).and_then(|mut watcher| {
        watcher
            .watch(Path::new(&workdir), RecursiveMode::Recursive)
            .map(|_| watcher)
    });

    match watcher {
        Ok(mut watcher) => {
            // 外部 gitdir 挂载失败不影响 workdir 主监听。
            for root in &meta.watch_roots {
                if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
                    eprintln!(
                        "workspace watcher: external git dir {} not watched: {error}",
                        root.display()
                    );
                }
            }
            let thread_workdir = workdir.clone();
            let spawned = thread::Builder::new()
                .name("dock-workspace-watch".to_string())
                .spawn(move || run_aggregator(thread_workdir, meta, rx, service));
            if let Err(error) = spawned {
                eprintln!("spawn workspace watch aggregator for {workdir} failed: {error}");
            }
            WatcherHandle {
                _watcher: Some(watcher),
                stop,
            }
        }
        Err(error) => {
            eprintln!(
                "workspace watcher for {workdir} failed ({error}); falling back to 2s sampling"
            );
            let poll_stop = Arc::clone(&stop);
            let git_dir = meta.git_dir;
            let spawned = thread::Builder::new()
                .name("dock-workspace-watch-poll".to_string())
                .spawn(move || run_poll_fallback(workdir, git_dir, poll_stop, service));
            if let Err(error) = spawned {
                eprintln!("spawn workspace watch poll fallback failed: {error}");
            }
            WatcherHandle {
                _watcher: None,
                stop,
            }
        }
    }
}

// ---- 事件聚合与分类 ----

#[derive(Default)]
struct ActivityBatch {
    fs: bool,
    git: bool,
    changed: BTreeSet<String>,
    truncated: bool,
}

impl ActivityBatch {
    fn is_empty(&self) -> bool {
        !self.fs && !self.git
    }

    fn note_path(&mut self, rel: String) {
        if self.changed.len() >= MAX_CHANGED_PATHS {
            self.truncated = true;
            return;
        }
        self.changed.insert(rel);
    }

    /// 分类规则：`.git/` 下或外部 gitdir 下的变更置 git；其余置 fs。
    fn absorb(
        &mut self,
        workdir: &Path,
        canonical_workdir: Option<&Path>,
        class_roots: &[PathBuf],
        event: notify::Result<Event>,
    ) {
        let event = match event {
            Ok(event) => event,
            Err(_) => {
                // watcher 自身报错（如队列溢出）：事件可能已丢，整个目录视为脏。
                self.fs = true;
                self.git = true;
                self.truncated = true;
                return;
            }
        };
        // 纯访问通知不产生状态变更。
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        for path in &event.paths {
            let rel = path
                .strip_prefix(workdir)
                .ok()
                .or_else(|| canonical_workdir.and_then(|prefix| path.strip_prefix(prefix).ok()))
                .map(|rel| rel.to_string_lossy().replace('\\', "/"));
            match rel {
                Some(rel) if !rel.is_empty() => {
                    if is_git_meta_rel(&rel) {
                        self.git = true;
                    } else {
                        self.fs = true;
                    }
                    self.note_path(rel);
                }
                // 外部 gitdir 的路径无法表达为 workdir 相对路径，只置 git 标志。
                _ if class_roots.iter().any(|root| path.starts_with(root)) => {
                    self.git = true;
                }
                _ => {
                    // 无法归因：宁可误报也不漏报。
                    self.fs = true;
                    self.git = true;
                    self.truncated = true;
                }
            }
        }
    }
}

fn is_git_meta_rel(rel: &str) -> bool {
    rel == ".git" || rel.starts_with(".git/")
}

fn run_aggregator(
    workdir: String,
    meta: GitMeta,
    rx: Receiver<notify::Result<Event>>,
    service: Weak<WorkspaceWatchService>,
) {
    let workdir_path = PathBuf::from(&workdir);
    // 部分后端（符号链接前缀后的 FSEvents）上报解析后的路径，留一个备选前缀。
    let canonical = std::fs::canonicalize(&workdir_path).ok();
    let canonical = canonical.filter(|resolved| resolved != &workdir_path);

    loop {
        // 先阻塞等 burst 的第一个事件，再在去抖窗口内持续吸收。
        let first = match rx.recv() {
            Ok(event) => event,
            Err(_) => return,
        };
        let mut batch = ActivityBatch::default();
        batch.absorb(
            &workdir_path,
            canonical.as_deref(),
            &meta.class_roots,
            first,
        );
        let window_end = Instant::now() + DEBOUNCE_WINDOW;
        let mut disconnected = false;
        loop {
            let now = Instant::now();
            if now >= window_end {
                break;
            }
            match rx.recv_timeout(window_end - now) {
                Ok(event) => batch.absorb(
                    &workdir_path,
                    canonical.as_deref(),
                    &meta.class_roots,
                    event,
                ),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if !batch.is_empty() {
            let Some(service) = service.upgrade() else {
                return;
            };
            service.emit_activity(
                &workdir,
                batch.fs,
                batch.git,
                batch.changed.into_iter().collect(),
                batch.truncated,
            );
        }
        if disconnected {
            return;
        }
    }
}

// ---- 轮询兜底 ----

#[derive(PartialEq, Eq)]
struct PollSample {
    workdir_mtime: Option<SystemTime>,
    head_mtime: Option<SystemTime>,
    index_mtime: Option<SystemTime>,
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

fn sample_workdir(workdir: &Path, git_dir: &Path) -> PollSample {
    PollSample {
        workdir_mtime: mtime_of(workdir),
        head_mtime: mtime_of(&git_dir.join("HEAD")),
        index_mtime: mtime_of(&git_dir.join("index")),
    }
}

fn run_poll_fallback(
    workdir: String,
    git_dir: PathBuf,
    stop: Arc<AtomicBool>,
    service: Weak<WorkspaceWatchService>,
) {
    let workdir_path = PathBuf::from(&workdir);
    let mut last = sample_workdir(&workdir_path, &git_dir);
    loop {
        // 分片 sleep，让 drop handle 后线程能及时退出。
        let interval_end = Instant::now() + POLL_INTERVAL;
        while Instant::now() < interval_end {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(POLL_STOP_CHECK);
        }
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Some(service) = service.upgrade() else {
            return;
        };
        let current = sample_workdir(&workdir_path, &git_dir);
        if current == last {
            continue;
        }
        let fs_changed = current.workdir_mtime != last.workdir_mtime;
        let git_changed =
            current.head_mtime != last.head_mtime || current.index_mtime != last.index_mtime;
        let mut changed_paths = Vec::new();
        if current.head_mtime != last.head_mtime {
            changed_paths.push(".git/HEAD".to_string());
        }
        if current.index_mtime != last.index_mtime {
            changed_paths.push(".git/index".to_string());
        }
        // 采样无法枚举工作区路径：workdir 变更时置 truncated。
        service.emit_activity(
            &workdir,
            fs_changed,
            git_changed || fs_changed,
            changed_paths,
            fs_changed,
        );
        last = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "kivio-dock-watch-{tag}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        std::fs::canonicalize(&dir).expect("canonicalize temp dir")
    }

    #[test]
    fn gitdir_file_pointer_parsing() {
        let dir = temp_dir("gitdir");
        let git_dir = dir.join("main").join(".git").join("worktrees").join("feat");
        std::fs::create_dir_all(&git_dir).expect("create gitdir");
        let workdir = dir.join("feat-worktree");
        std::fs::create_dir_all(&workdir).expect("create workdir");

        // 绝对路径指针。
        let dot_git = workdir.join(".git");
        std::fs::write(&dot_git, format!("gitdir: {}\n", git_dir.display())).expect("write .git");
        assert_eq!(
            resolve_gitdir_file(&dot_git),
            Some(std::fs::canonicalize(&git_dir).unwrap())
        );

        // 相对路径指针（锚在 .git 文件所在目录）。
        std::fs::write(&dot_git, "gitdir: ../main/.git/worktrees/feat\n").expect("write .git rel");
        assert_eq!(
            resolve_gitdir_file(&dot_git),
            Some(std::fs::canonicalize(&git_dir).unwrap())
        );

        // 非指针内容 / 空目标返回 None。
        std::fs::write(&dot_git, "not a pointer\n").expect("write junk");
        assert!(resolve_gitdir_file(&dot_git).is_none());
        std::fs::write(&dot_git, "gitdir:   \n").expect("write empty");
        assert!(resolve_gitdir_file(&dot_git).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_git_meta_attaches_external_gitdir_and_commondir() {
        let dir = temp_dir("meta");
        let common = dir.join("main").join(".git");
        let git_dir = common.join("worktrees").join("feat");
        std::fs::create_dir_all(&git_dir).expect("create gitdir");
        let workdir = dir.join("feat-worktree");
        std::fs::create_dir_all(&workdir).expect("create workdir");
        std::fs::write(
            workdir.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("write .git");
        std::fs::write(git_dir.join("commondir"), "../..\n").expect("write commondir");

        let meta = resolve_git_meta(&workdir);
        let canonical_git_dir = std::fs::canonicalize(&git_dir).unwrap();
        let canonical_common = std::fs::canonicalize(&common).unwrap();
        assert_eq!(meta.git_dir, canonical_git_dir);
        // gitdir 嵌套在 commondir 内：一个监听 root 覆盖两者。
        assert_eq!(meta.watch_roots, vec![canonical_common.clone()]);
        // 分类前缀长的（gitdir）在前。
        let gitdir_pos = meta
            .class_roots
            .iter()
            .position(|r| *r == canonical_git_dir)
            .expect("gitdir in class roots");
        let common_pos = meta
            .class_roots
            .iter()
            .position(|r| *r == canonical_common)
            .expect("commondir in class roots");
        assert!(gitdir_pos < common_pos);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn classifies_changed_paths() {
        let mut batch = ActivityBatch::default();
        let workdir = PathBuf::from("/repo");
        let class_roots = vec![PathBuf::from("/external/gitdir")];
        let mk = |paths: &[&str]| {
            let mut event = Event::new(EventKind::Modify(notify::event::ModifyKind::Any));
            for path in paths {
                event = event.add_path(PathBuf::from(path));
            }
            Ok::<Event, notify::Error>(event)
        };

        // 工作区文件 → fs；.git 内 → git；外部 gitdir → git（不记路径）。
        batch.absorb(&workdir, None, &class_roots, mk(&["/repo/src/main.rs"]));
        assert!(batch.fs && !batch.git);
        batch.absorb(&workdir, None, &class_roots, mk(&["/repo/.git/HEAD"]));
        assert!(batch.git);
        batch.absorb(
            &workdir,
            None,
            &class_roots,
            mk(&["/external/gitdir/index"]),
        );
        assert!(batch.git);
        assert!(batch
            .changed
            .iter()
            .all(|p| p == "src/main.rs" || p == ".git/HEAD"));

        // changedPaths 超 64 截断。
        let mut big = ActivityBatch::default();
        for i in 0..70 {
            big.note_path(format!("f{i}.txt"));
        }
        assert_eq!(big.changed.len(), 64);
        assert!(big.truncated);
    }
}
