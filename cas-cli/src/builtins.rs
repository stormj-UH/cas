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
    // session-learn (cas-39f5, EPIC cas-ebea): 7-signal session classifier
    // borrowed from third-brain-v5-skills. The skill body is also the
    // runtime prompt template embedded by the Stop hook handler (decision:
    // in-process for v1, see the skill body's "in-process vs subprocess"
    // section). v1 default: `[memory] session_learn_auto = false` —
    // manual-invocation only until user opts in.
    BuiltinFile {
        path: "skills/session-learn/SKILL.md",
        content: include_str!("builtins/skills/session-learn/SKILL.md"),
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
    // verify-before-claim skill (cas-5b2a, EPIC cas-ebea third-brain borrow).
    // Pre-close agent-discipline layer that forces workers to name, run, and
    // capture a proof command before claiming done. Advisory in v1; the
    // verification_store + close-gate.md self-checks remain the mechanical
    // gate underneath.
    BuiltinFile {
        path: "skills/verify-before-claim/SKILL.md",
        content: include_str!("builtins/skills/verify-before-claim/SKILL.md"),
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
    // session-learn (cas-39f5, EPIC cas-ebea) — Codex mirror. Kept
    // byte-identical to the .claude copy by regression test in
    // `test_session_learn_mirrors_are_identical`.
    BuiltinFile {
        path: "skills/session-learn/SKILL.md",
        content: include_str!("builtins/codex/skills/session-learn/SKILL.md"),
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
    // verify-before-claim skill (cas-5b2a) — codex mirror. See claude-side
    // entry above for context.
    BuiltinFile {
        path: "skills/verify-before-claim/SKILL.md",
        content: include_str!("builtins/codex/skills/verify-before-claim/SKILL.md"),
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

/// Outcome of a single `sync_builtin_detailed` call. The interesting
/// variant is `SkippedNotManaged` — that is the cas-4900 silent-skip
/// case (target exists, content differs from source, but the
/// managed-by-cas gate refused to write because neither side carries the
/// frontmatter marker). Callers that summarize a sync report should
/// surface these so the staleness becomes observable instead of silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Wrote a new file (target did not exist on disk).
    Created,
    /// Overwrote an existing file (content differed and the managed-by
    /// gate let us through).
    Updated,
    /// Target existed and content already matched source byte-for-byte.
    /// Happy-path no-op.
    Unchanged,
    /// Target exists, content differs from source, but neither version
    /// carries `managed_by: cas` in its frontmatter — the gate kept us
    /// from clobbering. **This is the visible-staleness signal**
    /// (cas-4900): the file at the destination is provably stale and
    /// the caller should surface it in CLI output.
    SkippedNotManaged,
}

impl SyncOutcome {
    /// True for the two write-bearing outcomes (`Created` / `Updated`).
    /// Preserves the back-compat surface for callers that previously
    /// read `sync_builtin` as a plain `bool`.
    pub fn wrote(self) -> bool {
        matches!(self, SyncOutcome::Created | SyncOutcome::Updated)
    }
}

/// Rich variant of [`sync_builtin`]: returns a [`SyncOutcome`] so the
/// caller can distinguish silent-skip (stale-source-not-managed) from
/// happy-path no-op, which the legacy `bool` return value collapsed
/// into the same value and produced the cas-4900 silent-staleness
/// regression.
pub fn sync_builtin_detailed(
    builtin: &BuiltinFile,
    target_dir: &Path,
) -> std::io::Result<SyncOutcome> {
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
            // Neither version is managed — don't overwrite user content.
            // Distinguish "content actually differs" (the silent-staleness
            // case worth warning about) from "content matches anyway"
            // (genuine no-op): emit `SkippedNotManaged` only on the
            // former so callers can warn-and-link the user to the
            // managed-by-cas marker fix.
            if existing == content {
                return Ok(SyncOutcome::Unchanged);
            }
            tracing::warn!(
                path = %builtin.path,
                "sync_builtin: silent skip — destination differs from source but \
                 neither side carries `managed_by: cas` frontmatter; file is stale. \
                 Add `managed_by: cas` to the source frontmatter to enable updates \
                 (cas-4900)."
            );
            return Ok(SyncOutcome::SkippedNotManaged);
        }

        // Check if content is the same
        if existing == content {
            return Ok(SyncOutcome::Unchanged);
        }

        std::fs::write(&target, content)?;
        Ok(SyncOutcome::Updated)
    } else {
        std::fs::write(&target, content)?;
        Ok(SyncOutcome::Created)
    }
}

/// Sync a built-in file to the target directory.
/// Returns true if file was written/updated.
///
/// Back-compat wrapper over [`sync_builtin_detailed`]; new call sites
/// should prefer the detailed variant so they can surface the
/// `SkippedNotManaged` case (cas-4900). Internal callers like
/// [`sync_all_builtins_inner`] already migrated.
pub fn sync_builtin(builtin: &BuiltinFile, target_dir: &Path) -> std::io::Result<bool> {
    Ok(sync_builtin_detailed(builtin, target_dir)?.wrote())
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
        match sync_builtin_detailed(builtin, target_dir)? {
            SyncOutcome::Created | SyncOutcome::Updated => {
                result.agents_updated += 1;
                result.updated_files.push(builtin.path.to_string());
            }
            SyncOutcome::SkippedNotManaged => {
                result.skipped_files.push(builtin.path.to_string());
            }
            SyncOutcome::Unchanged => {}
        }
    }

    // Sync skills
    for builtin in skills {
        match sync_builtin_detailed(builtin, target_dir)? {
            SyncOutcome::Created | SyncOutcome::Updated => {
                result.skills_updated += 1;
                result.updated_files.push(builtin.path.to_string());
            }
            SyncOutcome::SkippedNotManaged => {
                result.skipped_files.push(builtin.path.to_string());
            }
            SyncOutcome::Unchanged => {}
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
    /// Paths (relative to `target_dir`) whose source content differs from
    /// the on-disk destination, but the managed-by gate refused to
    /// overwrite because neither version carries `managed_by: cas`. This
    /// is the cas-4900 silent-staleness signal — callers like
    /// `cas update --sync` should surface these as warnings so the user
    /// can either add the marker to the source or accept the staleness
    /// knowingly. Distinct from "no-op" (`Unchanged`) where source and
    /// destination already match.
    pub skipped_files: Vec<String>,
}

impl SyncResult {
    pub fn total_updated(&self) -> usize {
        self.agents_updated + self.skills_updated
    }

    /// True when the sync left at least one file behind because the
    /// managed-by gate would not let us overwrite. cas-4900.
    pub fn has_silent_skips(&self) -> bool {
        !self.skipped_files.is_empty()
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

    /// cas-5787 (EPIC cas-ebea, third-brain borrow): both supervisor and
    /// worker skill bodies must document the "Context budgeting" 3-layer
    /// model so future maintainers see the framework before adding to the
    /// Immutable Core (this skill body). The section names the three
    /// layers explicitly (Immutable Core / Task Context / Ephemeral),
    /// cites the 12 KB ceiling, and points at the rationale memory file
    /// `project_session_start_truncation.md`. Both Claude and Codex
    /// mirrors are checked so neither surface silently drifts.
    #[test]
    fn test_skills_document_context_budgeting_cas_5787() {
        for (label, content) in [
            ("claude cas-supervisor.md", SUPERVISOR_GUIDE),
            ("claude cas-worker.md", WORKER_GUIDE),
            (
                "codex cas-supervisor.md",
                include_str!("builtins/codex/skills/cas-supervisor.md"),
            ),
            (
                "codex cas-worker.md",
                include_str!("builtins/codex/skills/cas-worker.md"),
            ),
        ] {
            for required in [
                "## Context budgeting",
                "Immutable Core",
                "Task Context",
                "Ephemeral",
                "project_session_start_truncation.md",
                "12 KB",
                "references/",
            ] {
                assert!(
                    content.contains(required),
                    "{label} missing required Context-budgeting marker: {required:?}"
                );
            }
        }
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
            // SKILL.md points workers at the gate (via close-gate.md).
            //
            // Historical note (cas-ec8f amendment): this loop previously also
            // asserted the literal substring "cas-code-review" was present in
            // cas-worker.md, but commit 8b82273 / cas-8962 deliberately
            // removed that mention when `[code_review] owner = "supervisor"`
            // became the default (v2.13.0+). Workers must NOT invoke
            // cas-code-review pre-close under the default ownership model —
            // the supervisor owns review timing at cherry-pick / EPIC-merge.
            // The assertion was silently failing on main from that commit
            // forward; cas-ec8f drops it here so the test reflects the
            // current ownership contract. The `close-gate.md` pointer is
            // still required — that doc is where the detailed gate content
            // lives and workers do need to know about it.
            for required in ["close-gate.md"] {
                assert!(
                    skill_content.contains(required),
                    "{label} cas-worker SKILL.md missing required marker: {required:?}"
                );
            }
            // close-gate.md carries the detailed gate content.
            //
            // Historical note (cas-ec8f amendment): this list previously
            // pinned five markers that documented the legacy worker-inline
            // code-review path: "Close-time Code Review Gate" (old section
            // title), "If close is blocked on P0" (legacy P0 hard-block
            // behavior), "bypass_code_review" (legacy worker bypass), plus
            // "cas-code-review" and "code-reviewer". Commit 167c57e
            // ("docs(skills): finish cas-5815 supervisor-default flip —
            // purge stale worker-runs-review prompts") deliberately rewrote
            // close-gate.md when `[code_review] owner = "supervisor"` became
            // the default — the inline-block markers no longer apply.
            // The assertions were silently failing on main from that point
            // forward. The new pin set encodes the *current* ownership
            // contract: close-gate.md documents the close gate, points
            // workers at cas-code-review with a "don't invoke pre-close"
            // caveat, and names the supervisor-owned default ownership flag.
            for required in [
                "Close Gate",
                "cas-code-review",
                "owner = \"supervisor\"",
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

    /// Extract the `description:` value from a SKILL.md frontmatter block.
    /// CAS skill descriptions are single-line YAML scalars (long, but a
    /// single physical line terminated by `\n`). Panics if the field is
    /// missing — every builtin SKILL.md is required to have one.
    #[cfg(test)]
    fn skill_description(content: &str) -> &str {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("description:") {
                return rest.trim_start();
            }
        }
        panic!("SKILL.md frontmatter missing required `description:` field");
    }

    #[test]
    fn test_cas_code_review_description_reflects_supervisor_owned_default() {
        // Regression for cas-ec8f. The skill's frontmatter description is
        // the FIRST thing the LLM sees when listing skills — when it
        // disagrees with the body, the description wins in practice. The
        // prior framing said "the pre-close quality gate for CAS factory
        // workers" and called `autofix` at `task.close` "the primary
        // path", which caused workers to self-dispatch personas at close
        // even under the v2.13.0+ default `[code_review] owner =
        // "supervisor"` (~100K input tokens burned per close, observed on
        // solid-cobra-88 cas-219d session log + reproduced on
        // daring-swan-93 cas-f645 in the same session this test was
        // added in).
        //
        // The new framing must: (a) not call autofix "the primary path";
        // (b) not describe this as a worker pre-close gate without the
        // supervisor-owned caveat; (c) explicitly name the supervisor as
        // the owner under the default model. Both BUILTIN_SKILLS (.claude
        // surface) and CODEX_BUILTIN_SKILLS (.codex surface) must agree
        // — the two are sync-mirrored by `cas update` and any drift
        // resurfaces the original bug on whichever harness reads stale
        // copy.
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/cas-code-review/SKILL.md")
                .unwrap_or_else(|| {
                    panic!("{label}: skills/cas-code-review/SKILL.md missing")
                });
            let description = skill_description(entry.content);

            // (a) `autofix` must not be framed as "the primary path".
            // The prior phrasing was literally "in `autofix` mode (the
            // primary path)" — we forbid the co-occurrence of those two
            // substrings, which is tight enough that any reasonable
            // phrasing that still framed autofix as primary would fail.
            assert!(
                !(description.contains("autofix") && description.contains("primary path")),
                "{label}: cas-code-review description still frames `autofix` as 'the primary path'. \
                 Under owner=\"supervisor\" (default since v2.13.0) the primary path is supervisor-driven \
                 interactive review at cherry-pick / EPIC-merge. Description: {description:?}",
            );

            // (b) "pre-close quality gate" is the other stale framing.
            // Allow the substring only if the description also names
            // the supervisor — i.e. only with proper context.
            let mentions_pre_close = description.contains("pre-close quality gate");
            let mentions_supervisor = description.contains("supervisor");
            assert!(
                !mentions_pre_close || mentions_supervisor,
                "{label}: cas-code-review description says 'pre-close quality gate' without naming \
                 the supervisor — workers will read it as a directive to self-dispatch personas at \
                 task.close. Description: {description:?}",
            );

            // (c) The description must affirmatively name supervisor
            // ownership. Without this, the absence of (a) and (b) is
            // not enough — a stripped-down description that just says
            // "code review orchestrator" still leaves workers free to
            // invoke it pre-close by default.
            assert!(
                mentions_supervisor,
                "{label}: cas-code-review description must explicitly name the supervisor as the \
                 default invoker so workers do not self-dispatch personas at task.close. \
                 Description: {description:?}",
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_session_learn() {
        // cas-39f5: session-learn must be registered in both surfaces so
        // `cas update` installs it at .claude/skills/session-learn/SKILL.md
        // (and the .codex equivalent). Without this entry the SKILL.md
        // source file exists on disk but never reaches downstream caches.
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            assert!(
                skills
                    .iter()
                    .any(|b| b.path == "skills/session-learn/SKILL.md"),
                "{label} missing session-learn SKILL.md registration"
            );
        }
    }

    #[test]
    fn test_session_learn_skill_covers_seven_signal_taxonomy() {
        // cas-39f5 AC: the skill body documents the 7-signal taxonomy
        // (concept, entity, correction, pattern, idea, decision, gap)
        // with each signal mapped to a CAS entry_type. The taxonomy is the
        // contract the Rust handler will encode in v2 — if a signal name
        // disappears from the skill body, the handler's JSON-schema parse
        // path silently drops findings of that type. Pin every signal name
        // so any drift triggers a compile-time test failure.
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/session-learn/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: session-learn SKILL.md not registered"));
            for signal in [
                "Concept",
                "Entity",
                "Correction",
                "Pattern",
                "Idea",
                "Decision",
                "Gap",
            ] {
                assert!(
                    entry.content.contains(&format!("**{signal}**")),
                    "{label}: session-learn SKILL.md missing signal marker **{signal}**"
                );
            }
            // Must also document the kill-switch flag so users can find it.
            assert!(
                entry.content.contains("session_learn_auto"),
                "{label}: session-learn SKILL.md must document the \
                 `session_learn_auto` kill-switch flag"
            );
            // And must record the in-process vs subprocess decision the
            // AC required.
            assert!(
                entry.content.contains("in-process"),
                "{label}: session-learn SKILL.md must document the \
                 in-process vs subprocess decision (cas-39f5 AC)"
            );
        }
    }

    #[test]
    fn test_session_learn_skill_md_mirrors_are_identical() {
        // cas-39f5: the .claude and .codex copies of session-learn/SKILL.md
        // are sync-mirrored by `cas update`. Drift between them silently
        // produces a different classifier prompt on whichever harness
        // reads the stale copy — exactly the failure mode cas-ec8f traced
        // in cas-code-review. Pin byte-identity at the source.
        let claude = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/session-learn/SKILL.md")
            .expect("BUILTIN_SKILLS missing session-learn SKILL.md");
        let codex = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/session-learn/SKILL.md")
            .expect("CODEX_BUILTIN_SKILLS missing session-learn SKILL.md");
        assert_eq!(
            claude.content, codex.content,
            "session-learn SKILL.md .claude and .codex copies must be byte-identical; \
             drift here produces a divergent classifier prompt across harnesses",
        );
    }

    #[test]
    fn test_cas_code_review_skill_md_mirrors_are_identical() {
        // The .claude and .codex builtin copies of cas-code-review/SKILL.md
        // are sync-mirrored by `cas update`. Drift between them
        // re-introduces the cas-ec8f regression on whichever harness reads
        // the stale copy — guard against that at the source.
        let claude = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-code-review/SKILL.md")
            .expect("BUILTIN_SKILLS missing cas-code-review SKILL.md");
        let codex = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-code-review/SKILL.md")
            .expect("CODEX_BUILTIN_SKILLS missing cas-code-review SKILL.md");
        assert_eq!(
            claude.content, codex.content,
            "cas-code-review SKILL.md .claude and .codex copies must be byte-identical; \
             drift here re-opens cas-ec8f on the harness reading the stale copy",
        );
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

    /// cas-4900 regression: `sync_all_builtins` was reported to silently
    /// skip reference files (anything under `<skill>/references/*.md`)
    /// when invoked against a project-style target_dir, even though the
    /// same code path worked against `~/.claude` (user-level). This test
    /// runs the same `sync_all_builtins` function against a tempdir that
    /// has been pre-populated with stale content for a reference file,
    /// asserts the stale content gets overwritten with fresh source, and
    /// asserts a separately-deleted reference file gets recreated.
    ///
    /// If this test PASSES on main, `sync_all_builtins` itself is innocent
    /// and the bug must live in the orchestration around it
    /// (`sync_claude_files` in `cli/update.rs`), most likely the
    /// `SkillSyncer::sync_all` invocation that runs immediately before.
    /// The locked-in assertion here is the safety net: any future
    /// refactor that breaks reference-file write logic at this layer
    /// fails this test loudly instead of slipping into silent staleness.
    #[test]
    fn test_sync_all_builtins_overwrites_stale_and_recreates_deleted_reference_files() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Initial sync — populate everything fresh.
        sync_all_builtins(&claude_dir).unwrap();

        // Pick two real reference files that exist in BUILTIN_SKILLS today.
        // Both carry `managed_by: cas` frontmatter (planning.md was the
        // exemplar in the 2026-05-06 cas-4900 repro).
        let planning_path = claude_dir.join("skills/cas-supervisor/references/planning.md");
        let close_gate_path = claude_dir.join("skills/cas-worker/references/close-gate.md");
        assert!(planning_path.exists(), "initial sync must have written planning.md");
        assert!(close_gate_path.exists(), "initial sync must have written close-gate.md");

        let planning_src = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/planning.md")
            .expect("planning.md must be registered in BUILTIN_SKILLS")
            .content;

        // Stage 1: overwrite planning.md with stale content (keep the
        // managed_by:cas frontmatter so the gate at sync_builtin:571
        // routes us into the write path).
        let stale_marker = "STALE CAS-4900 SENTINEL — should be overwritten on next sync";
        std::fs::write(
            &planning_path,
            format!("---\nname: planning\nmanaged_by: cas\n---\n\n{stale_marker}\n"),
        )
        .unwrap();

        // Stage 2: delete close-gate.md outright. The next sync must
        // recreate it from BUILTIN_SKILLS source.
        std::fs::remove_file(&close_gate_path).unwrap();
        assert!(!close_gate_path.exists(), "precondition: deletion took effect");

        // Re-run sync. This is the call that was reported to silently
        // no-op in per-project context.
        let result = sync_all_builtins(&claude_dir).unwrap();

        // Recreation invariant.
        assert!(
            close_gate_path.exists(),
            "cas-4900 regression: sync_all_builtins did NOT recreate the \
             deleted close-gate.md reference file"
        );
        let close_gate_after = std::fs::read_to_string(&close_gate_path).unwrap();
        assert!(
            close_gate_after.contains("managed_by: cas"),
            "recreated close-gate.md must carry the source frontmatter"
        );

        // Overwrite invariant.
        let planning_after = std::fs::read_to_string(&planning_path).unwrap();
        assert!(
            !planning_after.contains(stale_marker),
            "cas-4900 regression: sync_all_builtins did NOT overwrite the \
             stale planning.md reference file"
        );
        assert_eq!(
            planning_after, planning_src,
            "planning.md must match the BUILTIN_SKILLS source byte-for-byte after sync"
        );

        // Update count must reflect both files (recreated + overwritten).
        // Other built-ins were already current after the initial sync, so
        // the second-sync update count should be exactly 2.
        assert_eq!(
            result.total_updated(),
            2,
            "second sync should report exactly 2 updated files (the \
             recreated close-gate.md + the overwritten planning.md); got: {:?}",
            result.updated_files,
        );
    }

    /// cas-4900 surfacing: when the destination has an *unmanaged* file
    /// whose content differs from the source AND the source is also
    /// unmanaged, the gate correctly refuses to overwrite — but the
    /// outcome must be observable. Pin the `SkippedNotManaged` variant
    /// and the population of `SyncResult::skipped_files` so future
    /// refactors can't slip back into the pre-9362ee0 silent-skip mode.
    ///
    /// Note: with current `BUILTIN_SKILLS` content (post-9362ee0 — every
    /// builtin file carries `managed_by: cas`), this gate is effectively
    /// untriggerable in production via the real builtins. The test
    /// constructs a synthetic `BuiltinFile` whose source content lacks
    /// the marker so we can exercise the path. This is the regression
    /// safety net for the OTHER half of cas-4900 (the AC bullet
    /// "Reference files WITHOUT the marker either sync correctly OR
    /// emit a clear warning so silent-skip is no longer possible").
    #[test]
    fn test_sync_builtin_detailed_surfaces_silent_skip_for_unmanaged_drift() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let target_dir = temp.path();

        // Synthetic builtin whose source has NO managed_by marker — the
        // exact case the pre-9362ee0 gate would silently swallow.
        let synthetic = BuiltinFile {
            path: "skills/cas-test-synthetic/references/example.md",
            content: "# Synthetic ref file — unmanaged source\n\nupdated body\n",
        };

        // Seed destination with DIFFERENT unmanaged content. The gate
        // must refuse to overwrite (preserves user content) AND must
        // signal SkippedNotManaged so the caller can warn.
        let target_path = target_dir.join(synthetic.path);
        std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        std::fs::write(&target_path, "# Different unmanaged content\n").unwrap();

        let outcome = sync_builtin_detailed(&synthetic, target_dir).unwrap();
        assert_eq!(
            outcome,
            SyncOutcome::SkippedNotManaged,
            "drift between unmanaged source and unmanaged dest must surface as \
             SkippedNotManaged, not collapse into a silent false return"
        );
        assert!(
            !outcome.wrote(),
            "SkippedNotManaged must be a no-write outcome (preserves user content)"
        );

        // Identical unmanaged content → Unchanged (genuine no-op,
        // distinct from SkippedNotManaged so callers don't false-positive
        // warn on the happy path).
        std::fs::write(&target_path, synthetic.content).unwrap();
        let outcome = sync_builtin_detailed(&synthetic, target_dir).unwrap();
        assert_eq!(
            outcome,
            SyncOutcome::Unchanged,
            "matching unmanaged content must surface as Unchanged, not \
             SkippedNotManaged — surfacing it would noise up the warn channel"
        );
    }

    /// cas-4900 regression: `SyncResult::skipped_files` must be populated
    /// when the inner sync loop encounters a `SkippedNotManaged` outcome,
    /// and `has_silent_skips()` must report it. This is what the
    /// `cas update --sync` CLI surfacing reads from to print warnings.
    #[test]
    fn test_sync_result_tracks_silent_skips_for_cli_surfacing() {
        let mut result = SyncResult::default();
        assert!(!result.has_silent_skips());
        result.skipped_files.push("skills/foo/references/bar.md".to_string());
        assert!(
            result.has_silent_skips(),
            "any populated skipped_files entry must flip has_silent_skips() to true"
        );
        assert_eq!(result.skipped_files.len(), 1);
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
