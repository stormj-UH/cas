use crate::cli::hook::*;
use crate::cli::hook::config_gen::{get_cas_hooks_config, has_cas_hook_entries};
use crate::config::HookConfig;
use tempfile::TempDir;
use toml::map::Map;

#[test]
fn test_configure_creates_settings() {
    let temp = TempDir::new().unwrap();
    let result = configure_claude_hooks(temp.path(), false).unwrap();

    assert!(result); // Created new file
    assert!(temp.path().join(".claude/settings.json").exists());

    let content = std::fs::read_to_string(temp.path().join(".claude/settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&content).unwrap();

    if global_has_cas_hooks() {
        // Global hooks exist — project should NOT have hooks
        assert!(
            settings.get("hooks").is_none(),
            "Hooks should be omitted when global hooks exist"
        );
    } else {
        // No global hooks — project should have hooks in shell-form (/doctor compat, cas-c17b)
        assert!(settings.pointer("/hooks/SessionStart").is_some());
        assert!(settings.pointer("/hooks/SessionEnd").is_some());
        assert!(settings.pointer("/hooks/Stop").is_some());
        assert!(settings.pointer("/hooks/SubagentStop").is_some());
        assert!(settings.pointer("/hooks/PostToolUse").is_some());
        assert!(settings.pointer("/hooks/UserPromptSubmit").is_some());

        // Exec-form fixture: hook entries must carry "args" array, not a "command" string.
        // cas-9a60 re-adopts exec-form now that anthropics/claude-code#58441 is
        // fixed in CC ≥ 2.1.142 (verified on CC 2.1.143).
        let session_start_args = first_hook_args(&settings, "SessionStart");
        assert_eq!(
            session_start_args,
            Some(vec!["cas", "hook", "SessionStart"]),
            "cas init should emit exec-form args for SessionStart hook"
        );
        let stop_args = first_hook_args(&settings, "Stop");
        assert_eq!(
            stop_args,
            Some(vec!["cas", "hook", "Stop"]),
            "cas init should emit exec-form args for Stop hook"
        );
    }

    // Permissions should always be written
    let allow = settings
        .pointer("/permissions/allow")
        .expect("permissions.allow missing");
    let allow_arr = allow.as_array().expect("permissions.allow is not array");
    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Bash(cas :*)")),
        "Bash(cas :*) permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__task")),
        "mcp__cas__task permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__coordination")),
        "mcp__cas__coordination permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__memory")),
        "mcp__cas__memory permission missing"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__search")),
        "mcp__cas__search permission missing"
    );
}

#[test]
fn test_configure_merges_existing() {
    let temp = TempDir::new().unwrap();
    let claude_dir = temp.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();

    // Create existing settings with custom content
    let existing = serde_json::json!({
        "permissions": {
            "allow": ["Read", "Write"]
        },
        "hooks": {
            "CustomHook": [{"hooks": [{"type": "command", "command": "echo custom"}]}]
        }
    });
    std::fs::write(
        claude_dir.join("settings.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    // Configure CAS hooks
    let result = configure_claude_hooks(temp.path(), false).unwrap();
    assert!(!result); // Updated, not created

    let content = std::fs::read_to_string(claude_dir.join("settings.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&content).unwrap();

    if global_has_cas_hooks() {
        // Global hooks exist — CAS hooks should NOT be added to project
        assert!(
            settings.pointer("/hooks/SessionStart").is_none(),
            "CAS hooks should not be added when global hooks exist"
        );
        // Non-CAS custom hook should be preserved
        assert!(
            settings.pointer("/hooks/CustomHook").is_some(),
            "Non-CAS custom hooks should be preserved"
        );
    } else {
        // No global hooks — CAS hooks should be added
        assert!(settings.pointer("/hooks/SessionStart").is_some());
        assert!(settings.pointer("/hooks/Stop").is_some());
        assert!(settings.pointer("/hooks/PostToolUse").is_some());
    }

    // Existing permissions should always be preserved and CAS permissions added
    let allow = settings
        .pointer("/permissions/allow")
        .expect("permissions.allow missing");
    let allow_arr = allow.as_array().expect("permissions.allow is not array");

    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Read")),
        "Original Read permission should be preserved"
    );
    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Write")),
        "Original Write permission should be preserved"
    );
    assert!(
        allow_arr.iter().any(|v| v.as_str() == Some("Bash(cas :*)")),
        "Bash(cas :*) permission should be added"
    );
    assert!(
        allow_arr
            .iter()
            .any(|v| v.as_str() == Some("mcp__cas__task")),
        "mcp__cas__task permission should be added"
    );
}

#[test]
fn test_strip_cas_hooks() {
    let mut settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}],
            "SessionStart": [
                {"hooks": [{"type": "command", "command": "cas hook SessionStart"}]},
                {"hooks": [{"type": "command", "command": "cas factory check-staleness"}]}
            ],
            "CustomHook": [{"hooks": [{"type": "command", "command": "echo custom"}]}]
        },
        "permissions": {"allow": ["Read"]}
    });

    let modified = strip_cas_hooks(&mut settings);
    assert!(modified);

    // CAS hooks should be removed
    assert!(settings.pointer("/hooks/PreToolUse").is_none());
    assert!(settings.pointer("/hooks/SessionStart").is_none());

    // Non-CAS hook should be preserved
    assert!(settings.pointer("/hooks/CustomHook").is_some());

    // Permissions should be untouched
    assert!(settings.pointer("/permissions/allow").is_some());
}

#[test]
fn test_strip_cas_hooks_removes_empty_hooks_object() {
    let mut settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}]
        },
        "permissions": {"allow": ["Read"]}
    });

    strip_cas_hooks(&mut settings);

    // hooks object should be completely removed when empty
    assert!(settings.get("hooks").is_none());
    assert!(settings.get("permissions").is_some());
}

#[test]
fn test_has_cas_hook_entries() {
    let with_hooks = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}]
        }
    });
    assert!(has_cas_hook_entries(&with_hooks));

    let without_hooks = serde_json::json!({
        "hooks": {
            "Custom": [{"hooks": [{"type": "command", "command": "echo test"}]}]
        }
    });
    assert!(!has_cas_hook_entries(&without_hooks));

    let no_hooks = serde_json::json!({"permissions": {}});
    assert!(!has_cas_hook_entries(&no_hooks));
}

#[test]
fn test_configure_codex_creates_config() {
    let temp = TempDir::new().unwrap();
    let result = configure_codex_mcp_server(temp.path()).unwrap();

    assert!(result);
    let config_path = temp.path().join(".codex/config.toml");
    assert!(config_path.exists());

    let content = std::fs::read_to_string(&config_path).unwrap();
    let config: toml::Value = toml::from_str(&content).unwrap();
    let entry = config
        .get("mcp_servers")
        .and_then(|v| v.get("cas"))
        .and_then(|v| v.as_table())
        .expect("mcp_servers.cas missing");

    assert_eq!(
        entry.get("command"),
        Some(&toml::Value::String("cas".to_string()))
    );
    assert_eq!(
        entry.get("args"),
        Some(&toml::Value::Array(vec![toml::Value::String(
            "serve".to_string()
        )]))
    );
    assert_eq!(
        entry.get("env"),
        Some(&toml::Value::Table({
            let mut env = Map::new();
            env.insert(
                "CAS_CODEX_FALLBACK_SESSION".to_string(),
                toml::Value::String("1".to_string()),
            );
            env
        }))
    );
}

#[test]
fn test_configure_codex_updates_existing_entry() {
    let temp = TempDir::new().unwrap();
    let codex_dir = temp.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();

    let content = r#"
[mcp_servers.context7]
command = "cas"
args = ["old"]
env = { CAS_LOG = "debug" }
"#;
    std::fs::write(codex_dir.join("config.toml"), content).unwrap();

    let result = configure_codex_mcp_server(temp.path()).unwrap();
    assert!(result);

    let updated = std::fs::read_to_string(codex_dir.join("config.toml")).unwrap();
    let config: toml::Value = toml::from_str(&updated).unwrap();
    let entry = config
        .get("mcp_servers")
        .and_then(|v| v.get("context7"))
        .and_then(|v| v.as_table())
        .expect("mcp_servers.context7 missing");

    assert_eq!(
        entry.get("command"),
        Some(&toml::Value::String("cas".to_string()))
    );
    assert_eq!(
        entry.get("args"),
        Some(&toml::Value::Array(vec![toml::Value::String(
            "serve".to_string()
        )]))
    );
    assert_eq!(
        entry.get("env"),
        Some(&toml::Value::Table({
            let mut env = Map::new();
            env.insert(
                "CAS_LOG".to_string(),
                toml::Value::String("debug".to_string()),
            );
            env.insert(
                "CAS_CODEX_FALLBACK_SESSION".to_string(),
                toml::Value::String("1".to_string()),
            );
            env
        }))
    );
}

// Note: configure_mcp_server tests removed because they require the claude CLI
// which isn't available in test environments. The function now uses `claude mcp add`.

// =============================================================================
// Characterization tests for hook emission format (cas-9a60)
//
// cas-7ecd migrated emitters to exec-form "args" arrays.  cas-c17b reverted
// them back to shell-form "command" strings because Claude Code 2.1.139's
// /doctor validator rejected exec-form before the agent loaded, blocking every
// factory worker spawn (anthropics/claude-code#58441).
//
// cas-9a60 re-adopts exec-form now that #58441 is fixed in CC ≥ 2.1.142.
// exec-form remains recognised in has_cas_hook_entries / strip_cas_hooks for
// backward-compat with existing pre-revert settings.json files on disk.
// =============================================================================

/// Extract the first hook entry's "command" value for a given event name.
/// Returns None when the event is absent or the hook has no "command" key
/// (i.e. it is already using exec-form "args").
fn first_hook_command<'a>(config: &'a serde_json::Value, event: &str) -> Option<&'a str> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .iter()
        .find_map(|entry| {
            entry
                .get("hooks")?
                .as_array()?
                .iter()
                .find_map(|h| h.get("command")?.as_str())
        })
}

/// Extract the first hook entry's "args" array for a given event name.
/// Returns None when the event is absent or the hook has no "args" key.
fn first_hook_args<'a>(config: &'a serde_json::Value, event: &str) -> Option<Vec<&'a str>> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .iter()
        .find_map(|entry| {
            entry.get("hooks")?.as_array()?.iter().find_map(|h| {
                let args = h.get("args")?.as_array()?;
                Some(args.iter().filter_map(|v| v.as_str()).collect())
            })
        })
}

/// Extract the "command" value of the `idx`-th top-level hook registration
/// for a given event name (0-indexed).  Used to reach the second SessionStart
/// entry (`check-staleness`) which `first_hook_command` cannot reach.
fn nth_hook_command<'a>(
    config: &'a serde_json::Value,
    event: &str,
    idx: usize,
) -> Option<&'a str> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .get(idx)?
        .get("hooks")?
        .as_array()?
        .iter()
        .find_map(|h| h.get("command")?.as_str())
}

/// Extract the "args" array of the `idx`-th top-level hook registration
/// for a given event name (0-indexed).  Mirror of `nth_hook_command` for
/// exec-form entries that carry `"args"` instead of `"command"`.
fn nth_hook_args<'a>(
    config: &'a serde_json::Value,
    event: &str,
    idx: usize,
) -> Option<Vec<&'a str>> {
    config
        .get("hooks")?
        .get(event)?
        .as_array()?
        .get(idx)?
        .get("hooks")?
        .as_array()?
        .iter()
        .find_map(|h| {
            let args = h.get("args")?.as_array()?;
            Some(args.iter().filter_map(|v| v.as_str()).collect())
        })
}

/// Confirm: hooks emitted by get_cas_hooks_config use exec-form
/// `"args": ["cas", "hook", "<Event>"]`.  cas-9a60 re-adopts exec-form now
/// that anthropics/claude-code#58441 is fixed in CC ≥ 2.1.142.
#[test]
fn hook_entries_emit_exec_form_args() {
    let config = get_cas_hooks_config(&HookConfig::default());

    for (event, expected_args) in &[
        ("SessionStart", vec!["cas", "hook", "SessionStart"]),
        ("SessionEnd", vec!["cas", "hook", "SessionEnd"]),
        ("Stop", vec!["cas", "hook", "Stop"]),
        ("SubagentStart", vec!["cas", "hook", "SubagentStart"]),
        ("SubagentStop", vec!["cas", "hook", "SubagentStop"]),
        ("PostToolUse", vec!["cas", "hook", "PostToolUse"]),
        ("PreToolUse", vec!["cas", "hook", "PreToolUse"]),
        ("UserPromptSubmit", vec!["cas", "hook", "UserPromptSubmit"]),
        ("PermissionRequest", vec!["cas", "hook", "PermissionRequest"]),
        ("Notification", vec!["cas", "hook", "Notification"]),
        ("PreCompact", vec!["cas", "hook", "PreCompact"]),
    ] {
        assert_eq!(
            first_hook_args(&config, event),
            Some(expected_args.clone()),
            "{event} hook must carry exec-form args array (cas-9a60)"
        );
    }
}

/// Confirm: hooks emitted by get_cas_hooks_config do NOT use shell-form
/// `"command": "..."` — only exec-form `"args": [...]` is emitted (cas-9a60).
/// Shell-form hooks in existing user settings files are still recognised by
/// has_cas_hook_entries — see test_exec_form_still_detected_by_has_cas_hook_entries.
#[test]
fn hook_entries_do_not_emit_shell_form_command() {
    let config = get_cas_hooks_config(&HookConfig::default());

    for event in &[
        "SessionStart",
        "SessionEnd",
        "Stop",
        "SubagentStart",
        "SubagentStop",
        "PostToolUse",
        "PreToolUse",
        "UserPromptSubmit",
        "PermissionRequest",
        "Notification",
        "PreCompact",
    ] {
        assert_eq!(
            first_hook_command(&config, event),
            None,
            "{event} hook must not carry shell-form command string after cas-9a60 exec-form re-adoption"
        );
    }
}

/// AC#4 — has_cas_hook_entries still detects exec-form settings generated by
/// pre-cas-c17b CAS versions.  Users who ran `cas hook install` on older CAS
/// will have exec-form entries; detection must not regress.
#[test]
fn test_exec_form_still_detected_by_has_cas_hook_entries() {
    // Exec-form: realistic shape generated by CAS before cas-c17b (cas-7ecd era),
    // including matcher, timeout, and async fields as pre-cas-c17b CAS actually wrote.
    let exec_form = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Read|Write|Edit|Glob|Grep|Bash|NotebookEdit",
                "hooks": [{
                    "type": "command",
                    "args": ["cas", "hook", "PreToolUse"],
                    "timeout": 2000
                }]
            }]
        }
    });
    assert!(
        has_cas_hook_entries(&exec_form),
        "exec-form settings from pre-cas-c17b CAS must still be detected as CAS hooks"
    );

    // Shell-form: shape generated by CAS after cas-c17b
    let shell_form = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "cas hook PreToolUse"}]}]
        }
    });
    assert!(
        has_cas_hook_entries(&shell_form),
        "shell-form settings from post-cas-c17b CAS must also be detected as CAS hooks"
    );
}

/// Confirm: the second SessionStart hook entry emits `cas factory check-staleness`
/// in exec-form.  `first_hook_args` only returns the first entry; this test
/// uses `nth_hook_args(..., 1)` to reach the staleness-check entry explicitly.
/// cas-9a60 re-adopts exec-form for all emitters; this guards the
/// check-staleness emitter specifically (config_gen.rs).
#[test]
fn session_start_check_staleness_emits_exec_form() {
    let config = get_cas_hooks_config(&HookConfig::default());
    let staleness_args = nth_hook_args(&config, "SessionStart", 1);
    assert_eq!(
        staleness_args,
        Some(vec!["cas", "factory", "check-staleness"]),
        "check-staleness entry under SessionStart must use exec-form args (cas-9a60)"
    );
    // Also confirm the second entry has no shell-form command leak.
    let staleness_cmd = nth_hook_command(&config, "SessionStart", 1);
    assert!(
        staleness_cmd.is_none(),
        "check-staleness entry must not carry shell-form command string"
    );
}
