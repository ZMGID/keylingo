use super::*;
use std::{fs, io::Write};

fn skill(dir: &Path, text: &str) {
    fs::create_dir_all(dir.join("references")).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: example\ndescription: An example skill\n---\n{text}"),
    )
    .unwrap();
    fs::write(dir.join("references/guide.md"), text).unwrap();
}

#[test]
fn skill_install_replacement_keeps_backup_and_removes_obsolete_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join(".kivio/skills");
    skill(&source, "original");
    let installed = skill_install::install(&source, &root, false).unwrap();
    assert_eq!(installed["activationVerified"], false);
    fs::write(root.join("example/stale.txt"), "old").unwrap();
    skill(&source, "updated");
    assert!(skill_install::install(&source, &root, false).is_err());
    assert!(fs::read_to_string(root.join("example/SKILL.md"))
        .unwrap()
        .contains("original"));
    let updated = skill_install::install(&source, &root, true).unwrap();
    let backup = Path::new(updated["backup"].as_str().unwrap());
    assert!(!backup.starts_with(&root));
    assert!(backup.join("stale.txt").exists());
    assert!(!root.join("example/stale.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("example/references/guide.md")).unwrap(),
        "updated"
    );
}

fn zip(path: &Path, files: &[(&str, &str)]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, body) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn skill_zip_is_single_skill_bounded_and_failed_update_preserves_installation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join(".kivio/skills");
    skill(&source, "original");
    skill_install::install(&source, &root, false).unwrap();
    let archive = temp.path().join("incoming.zip");
    let md = "---\nname: example\ndescription: Description\n---\nnew";
    for bad in [
        "../escaped.txt",
        "/escaped.txt",
        "C:/escaped.txt",
        "folder\\escaped.txt",
    ] {
        zip(&archive, &[("example/SKILL.md", md), (bad, "escape")]);
        assert!(
            skill_install::install(&archive, &root, true).is_err(),
            "{bad}"
        );
        assert!(fs::read_to_string(root.join("example/SKILL.md"))
            .unwrap()
            .contains("original"));
    }
    zip(&archive, &[("one/SKILL.md", md), ("two/SKILL.md", md)]);
    assert!(skill_install::install(&archive, &root, true).is_err());
    zip(
        &archive,
        &[
            ("plugin/skills/example/SKILL.md", md),
            (
                "plugin/.kivio-plugin/plugin.json",
                r#"{"schemaVersion":1,"name":"plugin","version":"1"}"#,
            ),
        ],
    );
    assert!(skill_install::install(&archive, &root, true)
        .unwrap_err()
        .contains("plugin package"));
    zip(
        &archive,
        &[
            ("example/SKILL.md", md),
            ("example/references/a.md", "resource"),
            ("unrelated.txt", "outside"),
        ],
    );
    skill_install::install(&archive, &root, true).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("example/references/a.md")).unwrap(),
        "resource"
    );
    assert!(!root.join("example/unrelated.txt").exists());
}

#[test]
fn invalid_skill_does_not_replace_existing_skill() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let root = temp.path().join(".kivio/skills");
    skill(&source, "original");
    skill_install::install(&source, &root, false).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: example\n---\ninvalid").unwrap();
    assert!(skill_install::install(&source, &root, true).is_err());
    assert!(fs::read_to_string(root.join("example/SKILL.md"))
        .unwrap()
        .contains("original"));
}

#[test]
fn mcp_patch_preserves_credentials_and_unrelated_servers() {
    let mut settings = Settings::default();
    let id = upsert_server(&mut settings,None,&json!({"name":"helper","transport":"stdio","command":"node","args":["server.js"],"env":{"TOKEN":"secret"},"enabled":true})).unwrap();
    let other = upsert_server(
        &mut settings,
        None,
        &json!({"name":"other","transport":"streamable_http","url":"https://example.org/mcp"}),
    )
    .unwrap();
    assert!(settings.chat_tools.enabled);
    upsert_server(&mut settings, Some(&id), &json!({"enabled":false})).unwrap();
    let server = settings
        .chat_tools
        .servers
        .iter()
        .find(|s| s.id == id)
        .unwrap();
    assert_eq!(server.env["TOKEN"], "secret");
    assert_eq!(server.args, vec!["server.js"]);
    assert!(!server.enabled);
    assert!(settings.chat_tools.servers.iter().any(|s| s.id == other));
    assert!(upsert_server(&mut settings, Some("missing"), &json!({"enabled":true})).is_err());
    assert!(upsert_server(&mut settings, None, &json!({"mcpServers":{}})).is_err());
}

#[test]
fn mcp_owned_servers_and_credentials_cannot_be_overridden() {
    let mut settings = Settings::default();
    settings.chat_tools.servers.push(ChatMcpServer {
        id: "plugin-package-123-server".into(),
        name: "owned".into(),
        ..Default::default()
    });
    assert!(upsert_server(
        &mut settings,
        Some("plugin-package-123-server"),
        &json!({"enabled":true})
    )
    .is_err());
    assert!(upsert_server(&mut settings,None,&json!({"name":"http","transport":"streamable_http","url":"https://user:password@example.org"})).is_err());
    assert!(upsert_server(
        &mut settings,
        None,
        &json!({"name":"http","transport":"http","url":"https://example.org"})
    )
    .is_err());
    assert!(upsert_server(
        &mut settings,
        None,
        &json!({"id":"plugin-custom","name":"x"})
    )
    .is_err());
}

#[test]
fn inspection_omits_command_and_secret_fields_and_redacts_errors() {
    let mut settings = Settings::default();
    upsert_server(&mut settings,None,&json!({"name":"helper","command":"node","args":["--token=secret-argument"],"env":{"TOKEN":"private-token"},"headers":{"Authorization":"Bearer private-header"}})).unwrap();
    let summary = server_summary(&settings.chat_tools.servers[0]).to_string();
    assert!(!summary.contains("secret-argument"));
    assert!(!summary.contains("private-token"));
    assert!(!summary.contains("private-header"));
    assert!(summary.contains("TOKEN"));
    let sanitized = redact_text(&settings, "private-token Bearer private-header secret-argument https://user:password@example.org/mcp?token=url-secret".into());
    assert!(!sanitized.contains("private-"));
    assert!(!sanitized.contains("secret-argument"));
    assert!(!sanitized.contains("password"));
    assert!(!sanitized.contains("url-secret"));
    settings.chat_tools.servers[0]
        .env
        .insert("DEBUG".into(), "1".into());
    let output = result(&settings, json!({"count":1,"id":"uuid-1234","ready":true})).unwrap();
    let parsed: Value = serde_json::from_str(&output.content).unwrap();
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["id"], "uuid-1234");
}

#[test]
fn skill_settings_are_a_narrow_patch_and_unknown_actions_fail() {
    let mut settings = Settings::default();
    let native_before = settings.chat_tools.native_tools.run_command;
    apply_skill_settings(
        &mut settings,
        &json!({"skillRuntime":true,"skillAutoMatch":false}),
    )
    .unwrap();
    assert!(settings.chat_tools.native_tools.skill_runtime);
    assert!(!settings.chat_tools.skill_auto_match);
    assert_eq!(native_before, settings.chat_tools.native_tools.run_command);
    assert!(apply_skill_settings(&mut settings, &json!({"approvalPolicy":"auto"})).is_err());
    assert!(apply_skill_settings(&mut settings, &json!({"settings":{}})).is_err());
    assert!(apply_skill_settings(
        &mut settings,
        &json!({"skillScanPaths":["~/.claude/skills"]})
    )
    .is_err());
    assert!(serde_json::from_value::<Action>(json!({"action":"reset_all"})).is_err());
    assert!(serde_json::from_value::<Action>(
        json!({"action":"plugin_remove","id":"x","enabled":true})
    )
    .is_err());
}

#[test]
fn bundled_guides_parse_and_are_automatically_discoverable() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/skills");
    for name in [
        "kivio-configuration-guide",
        "kivio-diagnosing-skills",
        "kivio-diagnosing-plugins",
        "kivio-diagnosing-mcp",
        "kivio-diagnosing-hooks",
        "kivio-diagnosing-commands",
        "kivio-diagnosing-runtime",
    ] {
        let raw = fs::read_to_string(root.join(name).join("SKILL.md")).unwrap();
        let parsed = crate::skills::parse_skill_markdown(&raw, "builtin", None, vec![]).unwrap();
        assert_eq!(parsed.meta.id, name);
        assert!(!parsed.meta.disable_model_invocation);
        assert!(parsed.meta.description.chars().count() > 20);
        assert!(!parsed.body.is_empty());
    }
}
