//! Built-in CAS content that gets synced to .claude/ or .codex/ directories
//!
//! These definitions are managed by CAS and regenerated on `cas update`.
//! Files with `managed_by: cas` in frontmatter are overwritten on update.
//!
//! All content uses MCP tools (`mcp__cas__*`).
//!
//! The factory guide skill files are also the source of truth for HooksConfig
//! guidance that gets injected into supervisor/worker context.

use cas_mux::SupervisorCli;
use std::path::Path;

/// Factory supervisor guide - embedded at compile time (source of truth)
pub const SUPERVISOR_GUIDE: &str = include_str!("builtins/skills/cas-supervisor.md");

/// Factory worker guide - embedded at compile time (source of truth)
pub const WORKER_GUIDE: &str = include_str!("builtins/skills/cas-worker.md");

/// Shared skills preloaded into factory sessions
pub const TASK_TRACKING_GUIDE: &str = include_str!("builtins/skills/cas-task-tracking.md");
pub const MEMORY_GUIDE: &str = include_str!("builtins/skills/cas-memory-management/SKILL.md");
pub const SEARCH_GUIDE: &str = include_str!("builtins/skills/cas-search.md");
pub const CHECKLIST_GUIDE: &str = include_str!("builtins/skills/cas-supervisor-checklist.md");

/// A built-in file that CAS manages
pub struct BuiltinFile {
    /// Relative path within .claude/ (e.g., "agents/task-verifier.md")
    pub path: &'static str,
    /// File content (uses MCP tools)
    pub content: &'static str,
}

/// All built-in agents managed by CAS
pub const BUILTIN_AGENTS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "agents/task-verifier.md",
        content: include_str!("builtins/agents/task-verifier.md"),
    },
    BuiltinFile {
        path: "agents/learning-reviewer.md",
        content: include_str!("builtins/agents/learning-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/rule-reviewer.md",
        content: include_str!("builtins/agents/rule-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/duplicate-detector.md",
        content: include_str!("builtins/agents/duplicate-detector.md"),
    },
    BuiltinFile {
        path: "agents/session-summarizer.md",
        content: include_str!("builtins/agents/session-summarizer.md"),
    },
    // DEPRECATED (Phase 1 subsystem A, EPIC cas-0750): the legacy
    // `code-reviewer` agent is replaced by the `cas-code-review` multi-persona
    // skill. The entry is kept in BUILTIN_AGENTS only so `cas sync` overwrites
    // any downstream `.claude/agents/code-reviewer.md` with the deprecation
    // stub checked into the repo. Remove after downstream caches expire.
    BuiltinFile {
        path: "agents/code-reviewer.md",
        content: include_str!("builtins/agents/code-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/git-history-analyzer.md",
        content: include_str!("builtins/agents/git-history-analyzer.md"),
    },
    BuiltinFile {
        path: "agents/issue-intelligence-analyst.md",
        content: include_str!("builtins/agents/issue-intelligence-analyst.md"),
    },
];

/// All built-in agents managed by CAS for Codex
pub const CODEX_BUILTIN_AGENTS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "agents/task-verifier.md",
        content: include_str!("builtins/codex/agents/task-verifier.md"),
    },
    BuiltinFile {
        path: "agents/learning-reviewer.md",
        content: include_str!("builtins/codex/agents/learning-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/rule-reviewer.md",
        content: include_str!("builtins/codex/agents/rule-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/duplicate-detector.md",
        content: include_str!("builtins/codex/agents/duplicate-detector.md"),
    },
    BuiltinFile {
        path: "agents/session-summarizer.md",
        content: include_str!("builtins/codex/agents/session-summarizer.md"),
    },
    // DEPRECATED (Phase 1 subsystem A, EPIC cas-0750): see the note on the
    // claude-mirror entry above. Kept only so `cas sync` overwrites stale
    // downstream copies with the deprecation stub.
    BuiltinFile {
        path: "agents/code-reviewer.md",
        content: include_str!("builtins/codex/agents/code-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/factory-supervisor.md",
        content: include_str!("builtins/codex/agents/factory-supervisor.md"),
    },
    BuiltinFile {
        path: "agents/git-history-analyzer.md",
        content: include_str!("builtins/codex/agents/git-history-analyzer.md"),
    },
    BuiltinFile {
        path: "agents/issue-intelligence-analyst.md",
        content: include_str!("builtins/codex/agents/issue-intelligence-analyst.md"),
    },
];

/// All built-in skills managed by CAS
pub const BUILTIN_SKILLS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "skills/cas-memory-management/SKILL.md",
        content: include_str!("builtins/skills/cas-memory-management/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/schema.yaml",
        content: include_str!("builtins/skills/cas-memory-management/references/schema.yaml"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/body-templates.md",
        content: include_str!("builtins/skills/cas-memory-management/references/body-templates.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/overlap-detection.md",
        content: include_str!("builtins/skills/cas-memory-management/references/overlap-detection.md"),
    },
    BuiltinFile {
        path: "skills/cas-search/SKILL.md",
        content: include_str!("builtins/skills/cas-search.md"),
    },
    BuiltinFile {
        path: "skills/cas-task-tracking/SKILL.md",
        content: include_str!("builtins/skills/cas-task-tracking.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/SKILL.md",
        content: include_str!("builtins/skills/cas-supervisor.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/preflight.md",
        content: include_str!("builtins/skills/cas-supervisor/references/preflight.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/intake.md",
        content: include_str!("builtins/skills/cas-supervisor/references/intake.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/planning.md",
        content: include_str!("builtins/skills/cas-supervisor/references/planning.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/workflow.md",
        content: include_str!("builtins/skills/cas-supervisor/references/workflow.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/worker-recovery.md",
        content: include_str!("builtins/skills/cas-supervisor/references/worker-recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reference.md",
        content: include_str!("builtins/skills/cas-supervisor/references/reference.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/code-review-queue.md",
        content: include_str!(
            "builtins/skills/cas-supervisor/references/code-review-queue.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-supervisor-checklist/SKILL.md",
        content: include_str!("builtins/skills/cas-supervisor-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/SKILL.md",
        content: include_str!("builtins/skills/cas-worker.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/close-gate.md",
        content: include_str!("builtins/skills/cas-worker/references/close-gate.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/recovery.md",
        content: include_str!("builtins/skills/cas-worker/references/recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/details.md",
        content: include_str!("builtins/skills/cas-worker/references/details.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/SKILL.md",
        content: include_str!("builtins/skills/cas-brainstorm/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/handoff.md",
        content: include_str!("builtins/skills/cas-brainstorm/references/handoff.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/requirements-capture.md",
        content: include_str!("builtins/skills/cas-brainstorm/references/requirements-capture.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/SKILL.md",
        content: include_str!("builtins/skills/cas-ideate/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/references/post-ideation-workflow.md",
        content: include_str!("builtins/skills/cas-ideate/references/post-ideation-workflow.md"),
    },
    // cas-code-review (Phase 1 subsystem A, EPIC cas-0750).
    // Multi-persona code-review skill that replaces the legacy `code-reviewer`
    // agent. The old agent entry below is kept only to propagate a deprecation
    // stub via `cas sync`; all real functionality lives in this skill.
    BuiltinFile {
        path: "skills/cas-code-review/SKILL.md",
        content: include_str!("builtins/skills/cas-code-review/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/findings-schema.md",
        content: include_str!("builtins/skills/cas-code-review/references/findings-schema.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/correctness.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/correctness.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/testing.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/testing.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/maintainability.md",
        content: include_str!(
            "builtins/skills/cas-code-review/references/personas/maintainability.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/project-standards.md",
        content: include_str!(
            "builtins/skills/cas-code-review/references/personas/project-standards.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/security.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/security.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/performance.md",
        content: include_str!(
            "builtins/skills/cas-code-review/references/personas/performance.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/adversarial.md",
        content: include_str!(
            "builtins/skills/cas-code-review/references/personas/adversarial.md"
        ),
    },
    // fallow persona — 5th always-on reviewer. Thin Sonnet wrapper around
    // `fallow audit` that translates deterministic findings into the
    // ReviewerOutput envelope and self-skips on non-JS/TS repos / diffs.
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/fallow.md",
        content: include_str!(
            "builtins/skills/cas-code-review/references/personas/fallow.md"
        ),
    },
    // project-overview skill (EPIC cas-19a2b): generates
    // docs/PRODUCT_OVERVIEW.md for any project and writes a thin memory
    // pointer so CAS search surfaces the doc.
    BuiltinFile {
        path: "skills/project-overview/SKILL.md",
        content: include_str!("builtins/skills/project-overview/SKILL.md"),
    },
    // codemap skill (cas-4d84): remediation skill for the codemap
    // freshness gate. Generates .claude/CODEMAP.md so SessionStart and
    // PreToolUse stop nagging.
    BuiltinFile {
        path: "skills/codemap/SKILL.md",
        content: include_str!("builtins/skills/codemap/SKILL.md"),
    },
    // fallow skill: vendored from https://github.com/fallow-rs/fallow-skills
    // (MIT, Bart Waardenburg). Codebase intelligence for JS/TS — dead code,
    // duplication, complexity, boundaries, feature flags. SKILL.md +
    // 3 references match the upstream layout; only `managed_by: cas` is
    // injected so `cas sync` keeps user copies fresh.
    BuiltinFile {
        path: "skills/fallow/SKILL.md",
        content: include_str!("builtins/skills/fallow/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/cli-reference.md",
        content: include_str!("builtins/skills/fallow/references/cli-reference.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/gotchas.md",
        content: include_str!("builtins/skills/fallow/references/gotchas.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/patterns.md",
        content: include_str!("builtins/skills/fallow/references/patterns.md"),
    },
];

/// All built-in skills managed by CAS for Codex
pub const CODEX_BUILTIN_SKILLS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "skills/cas-memory-management/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-memory-management/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/schema.yaml",
        content: include_str!("builtins/codex/skills/cas-memory-management/references/schema.yaml"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/body-templates.md",
        content: include_str!("builtins/codex/skills/cas-memory-management/references/body-templates.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/overlap-detection.md",
        content: include_str!("builtins/codex/skills/cas-memory-management/references/overlap-detection.md"),
    },
    BuiltinFile {
        path: "skills/cas-search/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-search.md"),
    },
    BuiltinFile {
        path: "skills/cas-task-tracking/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-task-tracking.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-supervisor.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/preflight.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/preflight.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/intake.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/intake.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/planning.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/planning.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/workflow.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/workflow.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/worker-recovery.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/worker-recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reference.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/reference.md"),
    },
    BuiltinFile {
        path: "skills/cas-codex-supervisor-checklist/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-codex-supervisor-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-worker.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/close-gate.md",
        content: include_str!("builtins/codex/skills/cas-worker/references/close-gate.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/recovery.md",
        content: include_str!("builtins/codex/skills/cas-worker/references/recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/details.md",
        content: include_str!("builtins/codex/skills/cas-worker/references/details.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-brainstorm/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/handoff.md",
        content: include_str!("builtins/codex/skills/cas-brainstorm/references/handoff.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/requirements-capture.md",
        content: include_str!("builtins/codex/skills/cas-brainstorm/references/requirements-capture.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-ideate/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/references/post-ideation-workflow.md",
        content: include_str!("builtins/codex/skills/cas-ideate/references/post-ideation-workflow.md"),
    },
    // cas-code-review (Phase 1 subsystem A, EPIC cas-0750) — codex mirror.
    BuiltinFile {
        path: "skills/cas-code-review/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-code-review/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/findings-schema.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/findings-schema.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/correctness.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/correctness.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/testing.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/testing.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/maintainability.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/maintainability.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/project-standards.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/project-standards.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/security.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/security.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/performance.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/performance.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/adversarial.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/adversarial.md"
        ),
    },
    // fallow persona — codex mirror. See claude-side entry above.
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/fallow.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/fallow.md"
        ),
    },
    // project-overview skill (EPIC cas-19a2b) — codex mirror.
    BuiltinFile {
        path: "skills/project-overview/SKILL.md",
        content: include_str!("builtins/codex/skills/project-overview/SKILL.md"),
    },
    // codemap skill (cas-4d84) — codex mirror.
    BuiltinFile {
        path: "skills/codemap/SKILL.md",
        content: include_str!("builtins/codex/skills/codemap/SKILL.md"),
    },
    // fallow skill — codex mirror. See the claude-side entry above for the
    // upstream attribution (fallow-rs/fallow-skills, MIT).
    BuiltinFile {
        path: "skills/fallow/SKILL.md",
        content: include_str!("builtins/codex/skills/fallow/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/cli-reference.md",
        content: include_str!("builtins/codex/skills/fallow/references/cli-reference.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/gotchas.md",
        content: include_str!("builtins/codex/skills/fallow/references/gotchas.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/patterns.md",
        content: include_str!("builtins/codex/skills/fallow/references/patterns.md"),
    },
];

/// Check if a file is managed by CAS (has `managed_by: cas` in frontmatter)
pub fn is_managed_by_cas(content: &str) -> bool {
    // Check frontmatter for managed_by: cas
    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            let frontmatter = &content[3..3 + end];
            return frontmatter.contains("managed_by: cas")
                || frontmatter.contains("managed_by: \"cas\"");
        }
    }
    false
}

/// Preview what would change for a built-in file (dry-run)
/// Returns Some((old_content, new_content)) if file would be updated
pub fn preview_builtin(
    builtin: &BuiltinFile,
    target_dir: &Path,
) -> std::io::Result<Option<(String, String)>> {
    let target = target_dir.join(builtin.path);
    let content = builtin.content;

    if target.exists() {
        let existing = std::fs::read_to_string(&target)?;

        // Only update if managed by CAS
        if !is_managed_by_cas(&existing) && !is_managed_by_cas(content) {
            return Ok(None);
        }

        // Check if content is the same
        if existing == content {
            return Ok(None);
        }

        Ok(Some((existing, content.to_string())))
    } else {
        // New file
        Ok(Some((String::new(), content.to_string())))
    }
}

/// Sync a built-in file to the target directory
/// Returns true if file was written/updated
pub fn sync_builtin(builtin: &BuiltinFile, target_dir: &Path) -> std::io::Result<bool> {
    let target = target_dir.join(builtin.path);
    let content = builtin.content;

    // Create parent directories
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Check if file exists and whether we should overwrite
    if target.exists() {
        let existing = std::fs::read_to_string(&target)?;

        // Only overwrite if it's managed by CAS
        if !is_managed_by_cas(&existing) && !is_managed_by_cas(content) {
            // Neither version is managed - don't overwrite user content
            return Ok(false);
        }

        // Check if content is the same
        if existing == content {
            return Ok(false);
        }
    }

    std::fs::write(&target, content)?;
    Ok(true)
}

/// Sync all built-in files to the target directory
fn sync_all_builtins_inner(
    target_dir: &Path,
    agents: &[BuiltinFile],
    skills: &[BuiltinFile],
) -> std::io::Result<SyncResult> {
    let mut result = SyncResult::default();

    // Sync agents
    for builtin in agents {
        if sync_builtin(builtin, target_dir)? {
            result.agents_updated += 1;
            result.updated_files.push(builtin.path.to_string());
        }
    }

    // Sync skills
    for builtin in skills {
        if sync_builtin(builtin, target_dir)? {
            result.skills_updated += 1;
            result.updated_files.push(builtin.path.to_string());
        }
    }

    Ok(result)
}

/// Sync all built-in files to .claude/ directory
pub fn sync_all_builtins(claude_dir: &Path) -> std::io::Result<SyncResult> {
    sync_all_builtins_inner(claude_dir, BUILTIN_AGENTS, BUILTIN_SKILLS)
}

/// Sync all built-in files to .codex/ directory
pub fn sync_all_codex_builtins(codex_dir: &Path) -> std::io::Result<SyncResult> {
    sync_all_builtins_inner(codex_dir, CODEX_BUILTIN_AGENTS, CODEX_BUILTIN_SKILLS)
}

/// Sync all built-ins for a specific harness.
pub fn sync_all_builtins_for_harness(
    harness: SupervisorCli,
    target_dir: &Path,
) -> std::io::Result<SyncResult> {
    match harness {
        SupervisorCli::Claude => sync_all_builtins(target_dir),
        SupervisorCli::Codex => sync_all_codex_builtins(target_dir),
    }
}

#[derive(Default, Debug)]
pub struct SyncResult {
    pub agents_updated: usize,
    pub skills_updated: usize,
    pub updated_files: Vec<String>,
}

impl SyncResult {
    pub fn total_updated(&self) -> usize {
        self.agents_updated + self.skills_updated
    }
}

/// A pending builtin change for dry-run preview
#[derive(Debug)]
pub struct BuiltinChange {
    pub path: String,
    pub old_content: String,
    pub new_content: String,
    pub is_new: bool,
}

/// Preview all built-in file changes (dry-run mode)
pub fn preview_all_builtins(claude_dir: &Path) -> std::io::Result<Vec<BuiltinChange>> {
    let mut changes = Vec::new();

    let all_builtins = BUILTIN_AGENTS.iter().chain(BUILTIN_SKILLS.iter());

    for builtin in all_builtins {
        if let Some((old, new)) = preview_builtin(builtin, claude_dir)? {
            changes.push(BuiltinChange {
                path: builtin.path.to_string(),
                old_content: old.clone(),
                new_content: new,
                is_new: old.is_empty(),
            });
        }
    }

    Ok(changes)
}

/// Preview all Codex built-in file changes (dry-run mode)
pub fn preview_all_codex_builtins(codex_dir: &Path) -> std::io::Result<Vec<BuiltinChange>> {
    let mut changes = Vec::new();

    let all_builtins = CODEX_BUILTIN_AGENTS
        .iter()
        .chain(CODEX_BUILTIN_SKILLS.iter());

    for builtin in all_builtins {
        if let Some((old, new)) = preview_builtin(builtin, codex_dir)? {
            changes.push(BuiltinChange {
                path: builtin.path.to_string(),
                old_content: old.clone(),
                new_content: new,
                is_new: old.is_empty(),
            });
        }
    }

    Ok(changes)
}

/// Preview all built-ins for a specific harness.
pub fn preview_all_builtins_for_harness(
    harness: SupervisorCli,
    target_dir: &Path,
) -> std::io::Result<Vec<BuiltinChange>> {
    match harness {
        SupervisorCli::Claude => preview_all_builtins(target_dir),
        SupervisorCli::Codex => preview_all_codex_builtins(target_dir),
    }
}

// =============================================================================
// Factory Guidance Functions (for HooksConfig)
// =============================================================================

/// Extract the body content from a skill markdown file, stripping YAML frontmatter
///
/// Skill files have the format:
/// ```markdown
/// ---
/// name: skill-name
/// description: ...
/// ---
///
/// # Title
/// Content...
/// ```
///
/// This function returns everything after the closing `---` of the frontmatter.
pub fn extract_body(content: &str) -> &str {
    // Find the opening ---
    let Some(start) = content.find("---") else {
        return content;
    };

    // Find the closing --- (after the opening one)
    let after_first = &content[start + 3..];
    let Some(end_offset) = after_first.find("---") else {
        return content;
    };

    // Return everything after the closing ---
    let body_start = start + 3 + end_offset + 3;
    content[body_start..].trim_start()
}

/// Get the supervisor guidance injected at factory SessionStart.
///
/// Returns the trimmed supervisor SKILL.md plus the supervisor checklist.
/// task-tracking, memory, and search are autonomous skills the agent invokes
/// on demand via the Skill tool — bundling them inflated SessionStart context
/// past the harness "Output too large" threshold (the additionalContext got
/// truncated to a 2KB preview), which silently disabled the supervisor guide.
pub fn supervisor_guidance() -> String {
    [
        extract_body(SUPERVISOR_GUIDE),
        extract_body(CHECKLIST_GUIDE),
    ]
    .join("\n\n---\n\n")
}

/// Get the worker guidance injected at factory SessionStart.
///
/// Returns only the worker SKILL.md. task-tracking/memory/search load on
/// demand — same rationale as `supervisor_guidance`.
pub fn worker_guidance() -> String {
    extract_body(WORKER_GUIDE).to_string()
}

#[cfg(test)]
mod tests {
    use crate::builtins::*;

    #[test]
    fn test_extract_body_with_frontmatter() {
        let content = r#"---
name: test
description: A test skill
---

# Test Skill

This is the body content."#;

        let body = extract_body(content);
        assert!(body.starts_with("# Test Skill"));
        assert!(body.contains("This is the body content."));
        assert!(!body.contains("name: test"));
    }

    #[test]
    fn test_extract_body_no_frontmatter() {
        let content = "# Just Content\n\nNo frontmatter here.";
        let body = extract_body(content);
        assert_eq!(body, content);
    }

    #[test]
    fn test_supervisor_guidance_loads() {
        let guide = supervisor_guidance();
        assert!(guide.contains("Factory Supervisor"));
        assert!(!guide.contains("managed_by:"));
        assert!(
            guide.contains("Supervisor Checklist"),
            "should include checklist"
        );
        // task-tracking/memory/search are autonomous skills, not bundled.
        assert!(
            !guide.contains("CAS Task Tracking"),
            "should NOT bundle task-tracking — loads on demand"
        );
        assert!(
            !guide.contains("CAS Memory Management"),
            "should NOT bundle memory — loads on demand"
        );
    }

    /// SessionStart additionalContext gets truncated to a small preview by
    /// the Claude Code harness once the payload exceeds its threshold (the
    /// pre-trim bundle was ~61KB and got cut to 2KB). The exact threshold
    /// isn't documented; 12KB is a conservative ceiling that leaves room for
    /// the rest of the SessionStart context (codemap banner, agent identity,
    /// coordination block) to fit alongside without tripping truncation.
    #[test]
    fn test_supervisor_guidance_under_12kb() {
        let guide = supervisor_guidance();
        assert!(
            guide.len() < 12_288,
            "supervisor_guidance is {} bytes — over the 12KB ceiling. \
             Move content into cas-supervisor/references/ instead of \
             inlining it in cas-supervisor.md.",
            guide.len()
        );
    }

    #[test]
    fn test_worker_guidance_loads() {
        let guide = worker_guidance();
        assert!(guide.contains("Worker"));
        assert!(!guide.contains("managed_by:"));
        // Worker should NOT have supervisor checklist
        assert!(
            !guide.contains("Supervisor Checklist"),
            "should not include supervisor checklist"
        );
        // task-tracking/memory/search are autonomous skills, not bundled.
        assert!(
            !guide.contains("CAS Task Tracking"),
            "should NOT bundle task-tracking — loads on demand"
        );
    }

    /// Same rationale as `test_supervisor_guidance_under_12kb` — the worker
    /// SessionStart bundle must stay small enough that the harness doesn't
    /// truncate it to a preview. Move content into cas-worker/references/
    /// instead of inlining.
    #[test]
    fn test_worker_guidance_under_12kb() {
        let guide = worker_guidance();
        assert!(
            guide.len() < 12_288,
            "worker_guidance is {} bytes — over the 12KB ceiling. \
             Move content into cas-worker/references/ instead of \
             inlining it in cas-worker.md.",
            guide.len()
        );
    }

    #[test]
    fn test_is_managed_by_cas() {
        let managed = "---\nname: test\nmanaged_by: cas\n---\nContent";
        assert!(is_managed_by_cas(managed));

        let not_managed = "---\nname: test\n---\nContent";
        assert!(!is_managed_by_cas(not_managed));

        let no_frontmatter = "# Just content";
        assert!(!is_managed_by_cas(no_frontmatter));
    }

    #[test]
    fn test_builtin_agents_contains_git_history_analyzer() {
        assert!(
            BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/git-history-analyzer.md")
        );
        assert!(
            CODEX_BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/git-history-analyzer.md")
        );
    }

    #[test]
    fn test_builtin_agents_contains_issue_intelligence_analyst() {
        assert!(
            BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/issue-intelligence-analyst.md")
        );
        assert!(
            CODEX_BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/issue-intelligence-analyst.md")
        );
    }

    #[test]
    fn test_builtin_skills_contains_cas_brainstorm() {
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-brainstorm/SKILL.md")
        );
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-brainstorm/references/handoff.md")
        );
        assert!(BUILTIN_SKILLS.iter().any(
            |b| b.path == "skills/cas-brainstorm/references/requirements-capture.md"
        ));
        assert!(
            CODEX_BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-brainstorm/SKILL.md")
        );
    }

    #[test]
    fn test_builtin_skills_contains_cas_ideate() {
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-ideate/SKILL.md")
        );
        assert!(BUILTIN_SKILLS.iter().any(
            |b| b.path == "skills/cas-ideate/references/post-ideation-workflow.md"
        ));
        assert!(
            CODEX_BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-ideate/SKILL.md")
        );
    }

    #[test]
    fn test_cas_worker_skill_documents_code_review_gate() {
        // Phase 1 Subsystem A Unit 10 (EPIC cas-0750): the cas-worker
        // skill must document the new close-time code-review gate so
        // workers know how to read the block message, what happens to
        // residual findings, and which tools they must NOT fall back
        // to. After the cas-61af split, SKILL.md keeps the high-signal
        // references (cas-code-review and the close-gate pointer) and
        // the detailed P0/bypass/legacy-tool guidance lives in
        // references/close-gate.md. Pin both layers structurally so
        // drift through cas sync cannot silently delete them.
        for (label, skill_content, ref_content) in [
            (
                "claude",
                include_str!("builtins/skills/cas-worker.md"),
                include_str!("builtins/skills/cas-worker/references/close-gate.md"),
            ),
            (
                "codex",
                include_str!("builtins/codex/skills/cas-worker.md"),
                include_str!("builtins/codex/skills/cas-worker/references/close-gate.md"),
            ),
        ] {
            // SKILL.md points workers at the gate.
            for required in ["cas-code-review", "close-gate.md"] {
                assert!(
                    skill_content.contains(required),
                    "{label} cas-worker SKILL.md missing required marker: {required:?}"
                );
            }
            // close-gate.md carries the detailed gate content.
            for required in [
                "Close-time Code Review Gate",
                "If close is blocked on P0",
                "cas-code-review",
                "bypass_code_review",
                "code-reviewer",
            ] {
                assert!(
                    ref_content.contains(required),
                    "{label} cas-worker close-gate.md missing required marker: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_builtin_skills_contains_project_overview() {
        // EPIC cas-19a2b: project-overview SKILL.md must be registered so
        // `cas sync` installs it at .claude/skills/project-overview/SKILL.md.
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/project-overview/SKILL.md"),
            "skills/project-overview/SKILL.md missing from BUILTIN_SKILLS"
        );
        assert!(
            CODEX_BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/project-overview/SKILL.md"),
            "skills/project-overview/SKILL.md missing from CODEX_BUILTIN_SKILLS"
        );

        // Content sanity: frontmatter trigger phrases + required post-write
        // steps (memory pointer + freshness clear) must survive any drift.
        let entry = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/project-overview/SKILL.md")
            .unwrap();
        for required in [
            "name: project-overview",
            "managed_by: cas",
            "docs/PRODUCT_OVERVIEW.md",
            "<!-- keep -->",
            "mcp__cas__memory",
            "cas project-overview clear",
        ] {
            assert!(
                entry.content.contains(required),
                "project-overview SKILL.md missing required marker: {required:?}"
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_fallow() {
        // Vendored from fallow-rs/fallow-skills (MIT). SKILL.md plus three
        // references must be registered in both Claude and Codex mirrors so
        // `cas sync` installs the full skill.
        let expected = [
            "skills/fallow/SKILL.md",
            "skills/fallow/references/cli-reference.md",
            "skills/fallow/references/gotchas.md",
            "skills/fallow/references/patterns.md",
        ];
        for p in expected {
            assert!(
                BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from BUILTIN_SKILLS"
            );
            assert!(
                CODEX_BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from CODEX_BUILTIN_SKILLS"
            );
        }

        // Frontmatter sanity: `managed_by: cas` is the marker that lets
        // `cas sync` overwrite stale downstream copies, and the upstream
        // attribution must survive any drift from the vendor's repo.
        let entry = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/fallow/SKILL.md")
            .unwrap();
        for required in [
            "name: fallow",
            "managed_by: cas",
            "license: MIT",
            "author: Bart Waardenburg",
            "upstream: https://github.com/fallow-rs/fallow-skills",
        ] {
            assert!(
                entry.content.contains(required),
                "fallow SKILL.md missing required marker: {required:?}"
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_cas_code_review() {
        // Phase 1 subsystem A (EPIC cas-0750): 9 files per mirror; the
        // `fallow` persona added later brings the count to 10.
        let expected = [
            "skills/cas-code-review/SKILL.md",
            "skills/cas-code-review/references/findings-schema.md",
            "skills/cas-code-review/references/personas/correctness.md",
            "skills/cas-code-review/references/personas/testing.md",
            "skills/cas-code-review/references/personas/maintainability.md",
            "skills/cas-code-review/references/personas/project-standards.md",
            "skills/cas-code-review/references/personas/fallow.md",
            "skills/cas-code-review/references/personas/security.md",
            "skills/cas-code-review/references/personas/performance.md",
            "skills/cas-code-review/references/personas/adversarial.md",
        ];
        for p in expected {
            assert!(
                BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from BUILTIN_SKILLS"
            );
            assert!(
                CODEX_BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from CODEX_BUILTIN_SKILLS"
            );
        }
    }

    #[test]
    fn test_code_reviewer_agent_is_deprecation_stub() {
        // EPIC cas-0750: the legacy code-reviewer agent is replaced by the
        // cas-code-review skill. The file is kept in BUILTIN_AGENTS only to
        // propagate a deprecation stub via `cas sync`.
        for agents in [BUILTIN_AGENTS, CODEX_BUILTIN_AGENTS] {
            let entry = agents
                .iter()
                .find(|b| b.path == "agents/code-reviewer.md")
                .expect("code-reviewer.md must remain in the builtins list so sync overwrites downstream copies");
            assert!(
                entry.content.contains("deprecated: true"),
                "code-reviewer.md must carry `deprecated: true` in frontmatter"
            );
            assert!(
                entry.content.contains("replaced_by: cas-code-review"),
                "code-reviewer.md must name its replacement"
            );
            assert!(
                entry.content.contains("managed_by: cas"),
                "code-reviewer.md must keep `managed_by: cas` so sync overwrites stale copies"
            );
            assert!(
                entry.content.contains("DEPRECATED"),
                "code-reviewer.md must prominently mark itself as deprecated"
            );
        }
    }

    #[test]
    fn test_sync_installs_cas_code_review_and_overwrites_code_reviewer() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Pre-seed a stale copy of the old agent to prove sync overwrites it.
        let stale_agent = claude_dir.join("agents/code-reviewer.md");
        std::fs::create_dir_all(stale_agent.parent().unwrap()).unwrap();
        std::fs::write(
            &stale_agent,
            "---\nname: code-reviewer\nmanaged_by: cas\n---\nold content",
        )
        .unwrap();

        sync_all_builtins(&claude_dir).unwrap();

        for p in [
            "skills/cas-code-review/SKILL.md",
            "skills/cas-code-review/references/findings-schema.md",
            "skills/cas-code-review/references/personas/correctness.md",
            "skills/cas-code-review/references/personas/testing.md",
            "skills/cas-code-review/references/personas/maintainability.md",
            "skills/cas-code-review/references/personas/project-standards.md",
            "skills/cas-code-review/references/personas/security.md",
            "skills/cas-code-review/references/personas/performance.md",
            "skills/cas-code-review/references/personas/adversarial.md",
        ] {
            let f = claude_dir.join(p);
            assert!(f.exists(), "{p} not synced");
        }

        let overwritten = std::fs::read_to_string(&stale_agent).unwrap();
        assert!(
            overwritten.contains("DEPRECATED"),
            "sync must overwrite the stale code-reviewer.md with the deprecation stub"
        );
        assert!(
            overwritten.contains("replaced_by: cas-code-review"),
            "deprecation stub must name the replacement"
        );
    }

    #[test]
    fn test_builtin_agents_contains_task_verifier() {
        // Verify task-verifier agent is in BUILTIN_AGENTS and will be synced
        let has_task_verifier = BUILTIN_AGENTS
            .iter()
            .any(|b| b.path == "agents/task-verifier.md");
        assert!(
            has_task_verifier,
            "task-verifier.md must be in BUILTIN_AGENTS for cas init to sync it"
        );
    }

    #[test]
    fn test_task_verifier_has_correct_frontmatter() {
        // Verify task-verifier content has required frontmatter fields
        let task_verifier = BUILTIN_AGENTS
            .iter()
            .find(|b| b.path.contains("task-verifier"))
            .expect("task-verifier must exist in BUILTIN_AGENTS");

        assert!(
            task_verifier.content.contains("name: task-verifier"),
            "task-verifier must have name in frontmatter"
        );
        assert!(
            task_verifier.content.contains("managed_by: cas"),
            "task-verifier must be marked as managed by CAS"
        );
        assert!(
            task_verifier.content.contains("description:"),
            "task-verifier must have description"
        );
    }

    #[test]
    fn test_sync_all_builtins_includes_compound_engineering() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        sync_all_builtins(&claude_dir).unwrap();
        for p in [
            "agents/git-history-analyzer.md",
            "agents/issue-intelligence-analyst.md",
            "skills/cas-brainstorm/SKILL.md",
            "skills/cas-brainstorm/references/handoff.md",
            "skills/cas-brainstorm/references/requirements-capture.md",
            "skills/cas-ideate/SKILL.md",
            "skills/cas-ideate/references/post-ideation-workflow.md",
        ] {
            let f = claude_dir.join(p);
            assert!(f.exists(), "{} not synced", p);
            let body = std::fs::read_to_string(&f).unwrap();
            assert!(
                body.contains("managed_by: cas"),
                "{} missing managed_by: cas",
                p
            );
        }
    }

    #[test]
    fn test_sync_all_builtins_includes_agents() {
        // Verify sync_all_builtins syncs agents (which includes task-verifier)
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        let result = sync_all_builtins(&claude_dir).unwrap();

        // Should sync at least 1 agent (task-verifier)
        assert!(
            result.agents_updated > 0,
            "sync_all_builtins should sync agents"
        );

        // Verify task-verifier file was created
        let task_verifier_path = claude_dir.join("agents/task-verifier.md");
        assert!(
            task_verifier_path.exists(),
            "task-verifier.md should be created by sync_all_builtins"
        );
    }

    #[test]
    fn test_sync_all_codex_builtins_includes_agents() {
        // Verify sync_all_codex_builtins syncs agents (which includes task-verifier)
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();

        let result = sync_all_codex_builtins(&codex_dir).unwrap();

        // Should sync at least 1 agent (task-verifier)
        assert!(
            result.agents_updated > 0,
            "sync_all_codex_builtins should sync agents"
        );

        // Verify task-verifier file was created
        let task_verifier_path = codex_dir.join("agents/task-verifier.md");
        assert!(
            task_verifier_path.exists(),
            "task-verifier.md should be created by sync_all_codex_builtins"
        );
    }
}
