use std::collections::HashSet;

use serde_json::Value;

use super::{
    discover::build_registry_in,
    types::{SkillFileKind, SkillRecord, SkillRegistry},
};
use tauri::AppHandle;

#[derive(Default)]
pub struct SkillRunCache {
    activated: HashSet<String>,
    /// Registry scanned at most once per run (T1). `None` until the first skill
    /// tool call builds it; reused for every subsequent call in the same run.
    registry: Option<SkillRegistry>,
    /// 当前对话助手允许激活的技能 id 白名单(冻结自助手快照)。
    /// `None` = 无助手限制(全局行为);`Some(ids)` = 仅这些技能可激活,空集合 = 一个都不可。
    allowed_skill_ids: Option<Vec<String>>,
    /// 对话工作目录：扫描项目 `.kivio/skills` 与 `.agents/skills`（cwd → git 根）。
    project_cwd: Option<std::path::PathBuf>,
}

impl SkillRunCache {
    /// 设置助手技能白名单。在 run 启动时由 RunState 依据 assistant_snapshot 调用。
    pub fn set_allowed_skill_ids(&mut self, ids: Option<Vec<String>>) {
        self.allowed_skill_ids = ids;
    }

    pub fn set_project_cwd(&mut self, cwd: Option<std::path::PathBuf>) {
        self.project_cwd = cwd;
    }

    /// 某技能 id 是否在助手白名单内(无助手 = 不限)。
    pub fn skill_id_allowed(&self, skill_id: &str) -> bool {
        match &self.allowed_skill_ids {
            Some(ids) => ids.iter().any(|id| id == skill_id),
            None => true,
        }
    }

    /// Lazily build (once) and return the run-scoped skill registry. Subsequent
    /// calls reuse the cached registry instead of re-scanning the skill dirs.
    pub fn registry_for(
        &mut self,
        app: &AppHandle,
        scan_paths: &[String],
    ) -> Result<&SkillRegistry, String> {
        let project_cwd = self.project_cwd.clone();
        self.registry_or_build(|| {
            build_registry_in(app, scan_paths, project_cwd.as_deref())
        })
    }

    /// Build-once core of `registry_for`, factored out so the caching invariant
    /// is testable without an `AppHandle`. The builder runs only on cache miss.
    fn registry_or_build<F>(&mut self, build: F) -> Result<&SkillRegistry, String>
    where
        F: FnOnce() -> Result<SkillRegistry, String>,
    {
        if self.registry.is_none() {
            self.registry = Some(build()?);
        }
        Ok(self.registry.as_ref().expect("registry was just populated"))
    }

    pub fn activate_with_cache(&mut self, record: &SkillRecord) -> String {
        let key = record.meta.name.clone();
        if self.activated.contains(&key) {
            return format!(
                "Skill \"{}\" is already active in this run.\nSkill directory: {}",
                record.meta.name,
                record.base_dir.display()
            );
        }
        self.activated.insert(key);
        activate_skill(record)
    }
}

/// Substitute argument placeholders in a skill body.
///
/// - `$ARGUMENTS` → the full trailing argument string (everything after the
///   slash command), verbatim.
/// - `$ARG_NAME` → the positional word at the index of `ARG_NAME` in
///   `arg_names`. Words are whitespace-split from `args_raw`. A declared name
///   with no corresponding word substitutes to empty (never panics).
///
/// Unknown `$NAME` placeholders (not `ARGUMENTS`, not a declared arg) are left
/// untouched so skill bodies can mention literal `$` text safely.
pub fn substitute_arguments(body: &str, args_raw: &str, arg_names: &[String]) -> String {
    let trimmed = args_raw.trim();
    let words: Vec<&str> = trimmed.split_whitespace().collect();

    // Map of UPPERCASE declared name -> positional value (missing word => "").
    let mut name_values: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    for (index, name) in arg_names.iter().enumerate() {
        let key = name.trim();
        if key.is_empty() {
            continue;
        }
        name_values
            .entry(key.to_ascii_uppercase())
            .or_insert_with(|| words.get(index).copied().unwrap_or(""));
    }

    // Single left-to-right scan: every `$TOKEN` is resolved exactly once against the
    // original body. Substituted values are emitted verbatim and never re-scanned, so
    // a value containing a `$ARG_...`-like token is not re-substituted, and no token is
    // a prefix-collision victim of another (e.g. `$A` vs `$AB`).
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            // Copy a full UTF-8 char (find next char boundary).
            let mut end = i + 1;
            while end < bytes.len() && !body.is_char_boundary(end) {
                end += 1;
            }
            out.push_str(&body[i..end]);
            i = end;
            continue;
        }
        // Read the identifier after `$`: [A-Za-z0-9_]+ (ASCII only).
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_alphanumeric() || c == b'_' {
                end += 1;
            } else {
                break;
            }
        }
        if end == start {
            // Lone `$` (no identifier) → literal.
            out.push('$');
            i += 1;
            continue;
        }
        let ident = &body[start..end];
        let upper = ident.to_ascii_uppercase();
        if upper == "ARGUMENTS" {
            out.push_str(trimmed);
        } else if let Some(value) = name_values.get(&upper) {
            out.push_str(value);
        } else if let Some(stripped) = upper.strip_prefix("ARG_") {
            // `$ARG_NAME` convention: resolve the stripped name, else empty string.
            out.push_str(name_values.get(stripped).copied().unwrap_or(""));
        } else {
            // Unknown `$NAME` → leave literal so bodies can mention `$` text safely.
            out.push('$');
            out.push_str(ident);
        }
        i = end;
    }
    out
}

pub fn activate_skill(record: &SkillRecord) -> String {
    let mut out = format!(
        "<skill_content name=\"{}\">\n",
        xml_escape(&record.meta.name)
    );
    out.push_str(&record.body);
    out.push_str("\n\nSkill directory: ");
    out.push_str(&record.base_dir.display().to_string());
    out.push_str("\nRelative paths in this skill are relative to the skill directory.\n");

    if !record.meta.files.is_empty() {
        out.push_str("\n<skill_resources>\n");
        for file in &record.meta.files {
            out.push_str(&format!(
                "  <file kind=\"{}\">{}</file>\n",
                skill_file_kind_label(file.kind),
                xml_escape(&file.relative_path)
            ));
        }
        out.push_str("</skill_resources>\n");
    }
    out.push_str("</skill_content>");
    out
}

pub fn lookup_skill<'a>(registry: &'a SkillRegistry, name: &str) -> Option<&'a SkillRecord> {
    registry.find(name)
}

pub fn extract_skill_name(arguments: &Value) -> Result<String, String> {
    arguments
        .get("name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| "Skill name is required".to_string())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn skill_file_kind_label(kind: SkillFileKind) -> &'static str {
    match kind {
        SkillFileKind::SkillMd => "skillmd",
        SkillFileKind::Reference => "reference",
        SkillFileKind::Script => "script",
        SkillFileKind::Asset => "asset",
        SkillFileKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::{SkillMeta, SkillRecord};
    use super::*;
    use std::path::PathBuf;

    fn demo_meta() -> SkillMeta {
        SkillMeta {
            id: "demo".to_string(),
            name: "demo".to_string(),
            description: "Demo".to_string(),
            source: "user".to_string(),
            path: None,
            recommended_tools: vec![],
            disable_model_invocation: false,
            files: vec![],
            triggers: vec![],
            argument_hint: None,
            arguments: vec![],
        }
    }

    #[test]
    fn skill_run_cache_deduplicates_activate() {
        let record = SkillRecord {
            meta: demo_meta(),
            location: PathBuf::from("/skills/demo/SKILL.md"),
            base_dir: PathBuf::from("/skills/demo"),
            body: "Skill body".to_string(),
        };
        let mut cache = SkillRunCache::default();
        let first = cache.activate_with_cache(&record);
        assert!(first.contains("Skill body"));
        let second = cache.activate_with_cache(&record);
        assert!(second.contains("already active"));
        assert!(!second.contains("Skill body"));
    }

    #[test]
    fn skill_run_cache_builds_registry_once() {
        use std::cell::Cell;
        let mut cache = SkillRunCache::default();
        let builds = Cell::new(0u32);
        let build = || {
            builds.set(builds.get() + 1);
            Ok(SkillRegistry::default())
        };

        cache.registry_or_build(build).unwrap();
        cache.registry_or_build(build).unwrap();
        cache.registry_or_build(build).unwrap();

        assert_eq!(
            builds.get(),
            1,
            "registry must be built at most once per run"
        );
    }

    #[test]
    fn substitute_arguments_replaces_full_and_positional() {
        let body = "Commit with message: $ARGUMENTS\nFirst: $TITLE Second: $SCOPE";
        let out = substitute_arguments(
            body,
            "  fix login regression  ",
            &["title".to_string(), "scope".to_string()],
        );
        assert!(out.contains("Commit with message: fix login regression"));
        assert!(out.contains("First: fix Second: login"));
    }

    #[test]
    fn substitute_arguments_missing_positional_is_empty() {
        let body = "A=$FIRST B=$SECOND end";
        let out = substitute_arguments(body, "only", &["first".to_string(), "second".to_string()]);
        assert_eq!(out, "A=only B= end");
    }

    #[test]
    fn substitute_arguments_no_prefix_collision_between_a_and_ab() {
        // FIX 6 (1): `$ARG_A` must resolve to the `a` value and `$ARG_AB` to the `ab`
        // value; the old multi-pass impl corrupted `$ARG_AB` while replacing `$ARG_A`.
        let out = substitute_arguments(
            "$ARG_A|$ARG_AB",
            "x y",
            &["a".to_string(), "ab".to_string()],
        );
        assert_eq!(out, "x|y");
    }

    #[test]
    fn substitute_arguments_does_not_re_substitute_value_tokens() {
        // FIX 6 (2): a positional value that itself contains a `$ARG_...` token must be
        // emitted verbatim and never re-scanned/substituted by a later pass.
        let out = substitute_arguments(
            "first=$ARG_A second=$ARG_B",
            r#"$ARG_B payload"#,
            &["a".to_string(), "b".to_string()],
        );
        // word[0] == "$ARG_B", word[1] == "payload"; $ARG_A -> "$ARG_B" (literal),
        // $ARG_B -> "payload". The injected "$ARG_B" must survive unchanged.
        assert_eq!(out, "first=$ARG_B second=payload");
    }
}
