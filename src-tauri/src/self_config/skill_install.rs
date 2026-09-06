use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_BYTES: u64 = 100 * 1024 * 1024;
const MAX_FILES: usize = 10_000;

// Serializes installs, including simultaneous main conversations targeting one id.
static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) fn install(source: &Path, root: &Path, replace: bool) -> Result<Value, String> {
    let _guard = INSTALL_LOCK
        .lock()
        .map_err(|_| "Skill install lock unavailable")?;
    if is_link(source)? {
        return Err("Skill source cannot be a symbolic link or junction".into());
    }
    let source = fs::canonicalize(source).map_err(|e| e.to_string())?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    if root.starts_with(&source) {
        return Err("Source must not contain the installation root".into());
    }
    let owner = root.parent().ok_or("Skill root has no parent")?;
    // Staging/backups are siblings of skills/, never discoverable as duplicate skills.
    let staging_root = owner.join("skill-staging");
    fs::create_dir_all(&staging_root).map_err(|e| e.to_string())?;
    if is_link(&staging_root)? {
        return Err("Skill staging directory cannot be a link".into());
    }
    let stage = staging_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        if source.is_dir() {
            let mut remaining = MAX_BYTES;
            let mut count = 0;
            copy_tree(&source, &stage, &mut remaining, &mut count, 0)?;
        } else if source
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        {
            extract_zip(&source, &stage)?;
        } else {
            return Err("Expected a skill directory or a single-skill ZIP".into());
        }
        for manifest in [".kivio-plugin", ".codex-plugin", ".claude-plugin"] {
            if stage.join(manifest).join("plugin.json").exists() {
                return Err("This is a plugin package; use plugin_import".into());
            }
        }
        let skill_file = stage.join("SKILL.md");
        let raw = fs::read_to_string(&skill_file)
            .map_err(|_| "Skill root must contain a UTF-8 SKILL.md")?;
        let parsed = crate::skills::parse_skill_markdown(&raw, "user", None, vec![])?;
        if ["|", ">"].contains(&parsed.meta.description.trim()) {
            return Err("Kivio requires a single-line frontmatter description".into());
        }
        let dest = root.join(&parsed.meta.id);
        if source == dest {
            return Err(
                "Source already is the installed skill; edit it in place or use a separate source"
                    .into(),
            );
        }
        let mut backup: Option<PathBuf> = None;
        if dest.exists() || dest.is_symlink() {
            let actual = fs::canonicalize(&dest).map_err(|e| e.to_string())?;
            if actual.parent() != Some(root.as_path()) || is_link(&dest)? {
                return Err(
                    "Installed target must be a direct, non-linked child of the skill root".into(),
                );
            }
            if !replace {
                return Err(format!(
                    "Skill {} already exists; inspect it before using replace:true",
                    parsed.meta.id
                ));
            }
            let backups = owner.join("skill-backups");
            fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
            if is_link(&backups)? {
                return Err("Skill backup directory cannot be a link".into());
            }
            let path = backups.join(format!("{}-{}", parsed.meta.id, uuid::Uuid::new_v4()));
            fs::rename(&dest, &path).map_err(|e| e.to_string())?;
            backup = Some(path);
        }
        if let Err(error) = fs::rename(&stage, &dest) {
            if let Some(path) = &backup {
                fs::rename(path, &dest).map_err(|restore| {
                    format!(
                        "Install failed ({error}); restore failed ({restore}); backup: {}",
                        path.display()
                    )
                })?;
            }
            return Err(error.to_string());
        }
        Ok(
            json!({"id":parsed.meta.id,"name":parsed.meta.name,"path":dest.join("SKILL.md"),"backup":backup,"installed":true,"activationVerified":false}),
        )
    })();
    if stage.exists() {
        // Only delete the exact staging directory created by this call.
        if fs::canonicalize(&stage)
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            == fs::canonicalize(&staging_root).ok()
        {
            let _ = fs::remove_dir_all(&stage);
        }
    }
    result
}

fn is_link(path: &Path) -> Result<bool, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Ok(true);
        }
    }
    Ok(meta.file_type().is_symlink())
}

fn copy_tree(
    from: &Path,
    to: &Path,
    remaining: &mut u64,
    count: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 24 || is_link(from)? {
        return Err("Skill contains a link or exceeds directory depth 24".into());
    }
    for entry in fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == ".git" {
            continue;
        }
        *count += 1;
        if *count > MAX_FILES {
            return Err("Skill exceeds 10000 entries".into());
        }
        let src = entry.path();
        if is_link(&src)? {
            return Err("Skill directories cannot contain symbolic links or junctions".into());
        }
        let dst = to.join(entry.file_name());
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        if meta.is_dir() {
            fs::create_dir(&dst).map_err(|e| e.to_string())?;
            copy_tree(&src, &dst, remaining, count, depth + 1)?;
        } else if meta.is_file() {
            *remaining = remaining
                .checked_sub(meta.len())
                .ok_or("Skill exceeds 100 MiB")?;
            fs::copy(src, dst).map_err(|e| e.to_string())?;
        } else {
            return Err("Skill contains a non-regular file".into());
        }
    }
    Ok(())
}

fn zip_path(name: &str) -> Result<PathBuf, String> {
    if name.contains('\\') || name.contains(':') || name.contains('\0') {
        return Err("Unsafe ZIP path".into());
    }
    let path = Path::new(name);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("ZIP path escapes the skill".into());
    }
    Ok(path.to_path_buf())
}

fn extract_zip(source: &Path, stage: &Path) -> Result<(), String> {
    if fs::metadata(source).map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("Skill ZIP exceeds 100 MiB".into());
    }
    let mut archive = zip::ZipArchive::new(fs::File::open(source).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if archive.len() > MAX_FILES {
        return Err("Skill ZIP exceeds 10000 entries".into());
    }
    let mut roots = Vec::new();
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|e| e.to_string())?;
        let path = zip_path(file.name())?;
        if path.file_name().is_some_and(|name| name == "plugin.json")
            && path.parent().and_then(Path::file_name).is_some_and(|name| {
                [".kivio-plugin", ".codex-plugin", ".claude-plugin"]
                    .iter()
                    .any(|candidate| name == *candidate)
            })
        {
            return Err(
                "This ZIP is a plugin package; extract its package root and use plugin_import"
                    .into(),
            );
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("Skill ZIP contains a symbolic link".into());
        }
        if path.file_name().is_some_and(|n| n == "SKILL.md") {
            roots.push(path.parent().unwrap_or(Path::new("")).to_path_buf());
        }
    }
    if roots.len() != 1 {
        return Err("ZIP must contain exactly one SKILL.md; install each skill separately".into());
    }
    let mut remaining = MAX_BYTES;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let path = zip_path(file.name())?;
        let Ok(relative) = path.strip_prefix(&roots[0]) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let dest = stage.join(relative);
        if file.is_dir() {
            fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
            .map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut (&mut file).take(remaining + 1), &mut output)
            .map_err(|e| e.to_string())?;
        remaining = remaining
            .checked_sub(copied)
            .ok_or("Unpacked skill exceeds 100 MiB")?;
        output.flush().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = file.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dest, fs::Permissions::from_mode(mode & 0o777))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
