//! MCP Tools for CAS
//!
//! This module contains MCP tools organized by category:
//! - Memory tools (12): Entry management
//! - Task tools (15): Task and dependency management
//! - Rule tools (10): Rule management
//! - Skill tools (10): Skill management
//! - Search tools (1): Unified search with doc_type filter
//! - System tools (7): Context, stats, diagnostics, and utilities

use crate::hooks::{HookInput, build_context, handle_session_end, handle_session_start};
use crate::types::{
    BeliefType, ClaimResult, DEFAULT_LEASE_DURATION_SECS, Dependency, DependencyType, Entry,
    EntryType, LeaseStatus, MemoryTier, ObservationType, Priority, Rule, RuleStatus, Scope, Skill,
    SkillStatus, SkillType, Task, TaskStatus, TaskType, Verification, VerificationIssue,
    VerificationStatus, VerificationType, WorktreeStatus,
};
use cas_core::{DocType, SearchIndex, SearchOptions};

// Include all request types
mod types;
pub use types::*;

// CAS MCP service (7 meta-tools)
pub mod service;
pub use service::CasService;

// ============================================================================
// Tool Implementations - All in one impl block to satisfy the macro
// ============================================================================

// ============================================================================
// Sort Helper Functions
// ============================================================================

/// Sort any slice by task sort options, using a key function to extract the Task
fn sort_by_task_opts<T>(items: &mut [T], opts: &cas_types::TaskSortOptions, key: impl Fn(&T) -> &Task) {
    use cas_types::{SortOrder, TaskSortField};

    items.sort_by(|a, b| {
        let (a, b) = (key(a), key(b));
        let cmp = match opts.field {
            TaskSortField::Created => a.created_at.cmp(&b.created_at),
            TaskSortField::Updated => a.updated_at.cmp(&b.updated_at),
            TaskSortField::Priority => a.priority.0.cmp(&b.priority.0),
            TaskSortField::Title => a.title.cmp(&b.title),
        };
        match opts.effective_order() {
            SortOrder::Asc => cmp,
            SortOrder::Desc => cmp.reverse(),
        }
    });
}

/// Sort a vector of tasks based on sort options
pub(super) fn sort_tasks(tasks: &mut [Task], opts: &cas_types::TaskSortOptions) {
    sort_by_task_opts(tasks, opts, |t| t);
}

/// Sort a vector of blocked tasks (task, blockers) tuples based on sort options
pub(super) fn sort_blocked_tasks(
    blocked: &mut [(Task, Vec<Task>)],
    opts: &cas_types::TaskSortOptions,
) {
    sort_by_task_opts(blocked, opts, |(t, _)| t);
}

// ============================================================================
// Branch Name Helper
// ============================================================================

/// Convert a title to a branch-safe slug for epic branches
///
/// Creates branch names like `epic/add-user-authentication` from titles.
fn slugify_for_branch(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(50)
        .collect()
}

// ============================================================================
// Epic Merge Check Helper
// ============================================================================

/// Check for unmerged worker branches for an epic
///
/// In factory mode, workers may push branches in format `{epic-id}/{worker-name}` to a remote.
/// This function checks remote branches first, then falls back to local branches when no
/// matching remote branches exist.
///
/// Returns a list of unmerged branch names, or empty if all merged.
fn check_unmerged_epic_branches(epic_id: &str, target_branch: &str) -> Vec<String> {
    use std::collections::HashSet;

    let remote_branches = list_git_branches(
        None,
        &["branch", "-r", "--list", &format!("origin/{epic_id}/*")],
    );
    if !remote_branches.is_empty() {
        let mut merged: HashSet<String> =
            list_git_branches(None, &["branch", "-r", "--merged", target_branch])
                .into_iter()
                .collect();

        if merged.is_empty() && !target_branch.starts_with("origin/") {
            let fallback_branch = format!("origin/{target_branch}");
            merged = list_git_branches(None, &["branch", "-r", "--merged", &fallback_branch])
                .into_iter()
                .collect();
        }

        return remote_branches
            .into_iter()
            .filter(|b| !merged.contains(b))
            .collect();
    }

    let local_branches = list_git_branches(None, &["branch", "--list", &format!("{epic_id}/*")]);
    if local_branches.is_empty() {
        return vec![];
    }

    let merged_local: HashSet<String> =
        list_git_branches(None, &["branch", "--merged", target_branch])
            .into_iter()
            .collect();

    local_branches
        .into_iter()
        .filter(|b| !merged_local.contains(b))
        .collect()
}

/// Check how many commits behind its sync target a worktree is
///
/// Returns (commits_behind, sync_ref) or None if check fails
fn check_worktree_staleness(clone_path: &str) -> Option<(u32, String)> {
    use crate::worktree::GitOperations;
    use std::path::Path;
    use std::process::Command;

    let path = Path::new(clone_path);
    if !path.exists() {
        return None;
    }

    // Auto-detect target branch by checking current branch
    let branch_output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .ok()?;

    let current_branch = if branch_output.status.success() {
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string()
    } else {
        return None;
    };

    // Prefer upstream tracking ref, fall back to local parent/default branch.
    let sync_ref = if let Some(upstream) = current_upstream(path) {
        upstream
    } else if current_branch.starts_with("epic/") {
        current_branch.clone()
    } else if current_branch.starts_with("factory/") {
        list_git_branches(Some(path), &["branch", "--list", "epic/*"])
            .last()
            .cloned()
            .or_else(|| {
                GitOperations::detect_repo_root(path)
                    .ok()
                    .map(GitOperations::new)
                    .map(|git| git.detect_default_branch())
            })
            .unwrap_or_else(|| "main".to_string())
    } else {
        GitOperations::detect_repo_root(path)
            .ok()
            .map(GitOperations::new)
            .map(|git| git.detect_default_branch())
            .unwrap_or_else(|| "main".to_string())
    };

    // Fetch latest refs when sync target is a remote-tracking ref.
    if let Some(remote) = remote_for_ref(path, &sync_ref) {
        let _ = Command::new("git")
            .args(["fetch", &remote])
            .current_dir(path)
            .status();
    }

    // Check how many commits behind using git rev-list
    let output = Command::new("git")
        .args(["rev-list", "--count", &format!("HEAD..{sync_ref}")])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let behind_count = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .unwrap_or(0);

    Some((behind_count, sync_ref))
}

fn list_git_branches(path: Option<&std::path::Path>, args: &[&str]) -> Vec<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(path) = path {
        cmd.current_dir(path);
    }

    match cmd.output() {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(normalize_branch_line)
            .collect(),
        _ => vec![],
    }
}

fn normalize_branch_line(line: &str) -> Option<String> {
    let trimmed = line
        .trim()
        .trim_start_matches('*')
        .trim_start_matches('+')
        .trim();
    if trimmed.is_empty() || trimmed.contains(" -> ") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn current_upstream(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let upstream = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if upstream.is_empty() {
        None
    } else {
        Some(upstream)
    }
}

fn remote_for_ref(path: &std::path::Path, reference: &str) -> Option<String> {
    let candidate = reference.split('/').next()?;
    if candidate.is_empty() {
        return None;
    }

    let output = std::process::Command::new("git")
        .args(["remote"])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let remotes = String::from_utf8_lossy(&output.stdout);
    remotes
        .lines()
        .map(str::trim)
        .find(|name| *name == candidate)
        .map(str::to_string)
}

// NOTE: #[tool_router] removed - CasService (service/mod.rs) is the actual MCP service.
// CasCore methods are called directly, not through tool routing.
// This reduces compile time by avoiding proc-macro expansion of ~77 tools.

/// Helper to truncate strings for display (shared by core and service modules)
pub(crate) fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

pub(crate) mod core;

#[cfg(test)]
mod mod_tests;
