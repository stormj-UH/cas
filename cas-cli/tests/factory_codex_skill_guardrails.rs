use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cas-cli must live under repo root")
        .to_path_buf()
}

fn load(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e))
}

#[test]
fn codex_factory_skills_use_cs_prefix_only() {
    let root = repo_root();
    let skills_dir = root.join(".codex/skills");
    if !skills_dir.exists() {
        return;
    }

    let entries = fs::read_dir(&skills_dir).expect("read .codex/skills");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("cas-factory-") {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }

        let content = load(&skill_path);
        let has_mcp_examples = content.contains("mcp__cs__") || content.contains("mcp__cas__");
        if has_mcp_examples {
            assert!(
                content.contains("mcp__cs__"),
                "{} should include mcp__cs__ examples",
                skill_path.display()
            );
        }
        assert!(
            !content.contains("mcp__cas__"),
            "{} still contains legacy mcp__cas__ references",
            skill_path.display()
        );
        assert!(
            !content.contains("action=prompt"),
            "{} still contains legacy action=prompt usage",
            skill_path.display()
        );
    }
}

#[test]
fn codex_builtin_supervisor_guide_includes_core_workflow() {
    let root = repo_root();
    let guide = root.join("cas-cli/src/builtins/codex/skills/cas-supervisor.md");
    let content = load(&guide);

    assert!(
        content.contains("spawn_workers"),
        "supervisor guide should include spawn_workers"
    );
    assert!(
        content.contains("Never implement tasks yourself"),
        "supervisor guide should include hard rule about not implementing"
    );
    assert!(
        content.contains("mcp__cs__"),
        "codex supervisor guide should use mcp__cs__ prefix"
    );
}

#[test]
fn supervisor_skill_mirrors_include_implementation_unit_template() {
    // After cas-61af split cas-supervisor.md into a main file + references,
    // the Implementation Unit Template moved to planning.md. The guardrail
    // checks that file instead (both .claude and .codex trees must match).
    let root = repo_root();
    let claude = load(
        &root.join("cas-cli/src/builtins/skills/cas-supervisor/references/planning.md"),
    );
    let codex = load(
        &root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/planning.md"),
    );

    for (label, content) in [("claude", &claude), ("codex", &codex)] {
        assert!(
            content.contains("## Implementation Unit Template"),
            "{label} planning.md missing '## Implementation Unit Template' heading"
        );
        // Canonical template markers (R1)
        for marker in [
            "**Unit N: [Name]**",
            "**Goal:**",
            "**Requirements:**",
            "**Dependencies:**",
            "**Files:**",
            "**Approach:**",
            "**Execution note:**",
            "**Patterns to follow:**",
            "**Test scenarios:**",
            "**Verification:**",
        ] {
            assert!(
                content.contains(marker),
                "{label} planning.md template missing marker: {marker}"
            );
        }
        // R4 mapping table
        assert!(
            content.contains("| Template field | Maps to |"),
            "{label} planning.md missing template→task schema mapping table"
        );
        // R6/R7 scope note
        assert!(
            content.contains("EPIC subtasks"),
            "{label} planning.md missing EPIC-subtasks-only scope note"
        );
        // R13 cross-link: Spec Requirements section mentions the template
        let spec_idx = content
            .find("## Spec Requirements")
            .unwrap_or_else(|| panic!("{label} missing Spec Requirements heading"));
        let tmpl_idx = content
            .find("## Implementation Unit Template")
            .unwrap_or_else(|| panic!("{label} missing Implementation Unit Template heading"));
        let spec_block = &content[spec_idx..tmpl_idx];
        assert!(
            spec_block.contains("Implementation Unit Template"),
            "{label} Spec Requirements section missing cross-link to Implementation Unit Template"
        );
    }
}

#[test]
fn codex_worker_runtime_instruction_allows_close_then_escalate() {
    let root = repo_root();
    let pty_rs = root.join("crates/cas-pty/src/pty.rs");
    let content = load(&pty_rs);

    assert!(
        content.contains("close with `mcp__cs__task action=close"),
        "runtime worker instruction should instruct workers to close tasks"
    );
    assert!(
        !content.contains("DO NOT close the task yourself"),
        "runtime worker instruction should not forbid close universally"
    );
}
