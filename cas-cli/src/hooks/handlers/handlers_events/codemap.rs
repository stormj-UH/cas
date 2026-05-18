use std::path::Path;

use crate::hooks::handlers::*;

/// Represents a structural file change detected from a git commit
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodemapChange {
    /// Change type: "A" (added), "D" (deleted), "R" (renamed)
    #[serde(rename = "type")]
    pub change_type: String,
    /// File path affected
    pub path: String,
    /// For renames, the old path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

/// A single pending codemap entry (JSONL format - one per line)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodemapPending {
    pub changes: Vec<CodemapChange>,
    pub commit: String,
    pub recorded_at: String,
}

/// Path to the codemap pending file relative to cas_root
const CODEMAP_PENDING_FILE: &str = "codemap-pending.json";

/// Detect structural file changes (add/delete/rename) from a successful git commit.
///
/// Called from PostToolUse for Bash commands containing "git commit" that exit 0.
/// Writes changes to `.cas/codemap-pending.json` in JSONL format (append-safe).
///
/// Pattern follows `detect_and_link_git_commit` in attribution.rs: same trigger,
/// same silent-failure error handling.
pub fn detect_codemap_structural_changes(cas_root: &Path, input: &HookInput) {
    // Get tool input
    let tool_input = match &input.tool_input {
        Some(ti) => ti,
        None => return,
    };

    // Check if this is a git commit command
    let command = match tool_input.get("command").and_then(|v| v.as_str()) {
        Some(cmd) => cmd,
        None => return,
    };

    if !super::attribution::is_git_commit_command(command) {
        return;
    }

    // Check for successful exit
    let tool_response = match &input.tool_response {
        Some(tr) => tr,
        None => return,
    };

    let exit_code = tool_response
        .get("exitCode")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);
    if exit_code != 0 {
        return;
    }

    // Get the commit hash from stdout
    let stdout = tool_response
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let commit_hash = match super::attribution::extract_commit_hash(stdout) {
        Some(hash) => hash,
        None => return,
    };

    // Run git diff-tree to find structural changes (A/D/R only)
    let changes = match get_structural_changes(&commit_hash) {
        Some(c) if !c.is_empty() => c,
        _ => return, // No structural changes or error
    };

    // Write to codemap-pending.json (JSONL append)
    let pending = CodemapPending {
        changes,
        commit: commit_hash,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };

    let pending_path = cas_root.join(CODEMAP_PENDING_FILE);

    // Serialize as single-line JSON for JSONL format
    let line = match serde_json::to_string(&pending) {
        Ok(json) => format!("{json}\n"),
        Err(_) => return,
    };

    // Append to file (create if doesn't exist)
    use std::io::Write;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&pending_path);

    match file {
        Ok(mut f) => {
            let _ = f.write_all(line.as_bytes());
        }
        Err(_) => {} // Silent failure - best effort
    }
}

/// Run `git diff-tree --name-status -r HEAD~1 HEAD` and parse structural changes.
///
/// Only returns Added (A), Deleted (D), and Renamed (R) entries.
/// Returns None on git command failure.
fn get_structural_changes(commit_hash: &str) -> Option<Vec<CodemapChange>> {
    // Use the specific commit to diff against its parent
    let output = std::process::Command::new("git")
        .args([
            "diff-tree",
            "--name-status",
            "-r",
            "--no-commit-id",
            &format!("{commit_hash}~1"),
            commit_hash,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        // Fallback for initial commit (no parent)
        let output = std::process::Command::new("git")
            .args([
                "diff-tree",
                "--name-status",
                "-r",
                "--no-commit-id",
                "--root",
                commit_hash,
            ])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        return parse_diff_tree_output(&String::from_utf8_lossy(&output.stdout));
    }

    parse_diff_tree_output(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `git diff-tree --name-status` output into structural changes.
///
/// Only keeps A (added), D (deleted), and R (renamed) entries.
/// Ignores M (modified) and other change types.
fn parse_diff_tree_output(output: &str) -> Option<Vec<CodemapChange>> {
    let mut changes = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }

        let status = parts[0];
        match status {
            "A" => {
                changes.push(CodemapChange {
                    change_type: "A".to_string(),
                    path: parts[1].to_string(),
                    old_path: None,
                });
            }
            "D" => {
                changes.push(CodemapChange {
                    change_type: "D".to_string(),
                    path: parts[1].to_string(),
                    old_path: None,
                });
            }
            s if s.starts_with('R') && parts.len() >= 3 => {
                // Rename: R100\told_path\tnew_path
                changes.push(CodemapChange {
                    change_type: "R".to_string(),
                    path: parts[2].to_string(),
                    old_path: Some(parts[1].to_string()),
                });
            }
            _ => {} // Ignore M (modified), C (copied), etc.
        }
    }

    Some(changes)
}

/// Staleness severity for codemap freshness checks.
#[derive(Debug, Clone, PartialEq)]
pub enum CodemapStaleness {
    /// CODEMAP.md does not exist at all
    Missing,
    /// Stale with fewer than 10 structural changes (informational)
    Stale {
        total_changes: usize,
        file_list: Vec<String>,
        commit_info: String,
    },
    /// Stale with 10+ structural changes (urgent)
    SignificantlyStale {
        total_changes: usize,
        file_list: Vec<String>,
        commit_info: String,
    },
}

/// Threshold for "significantly stale" — 10+ structural changes warrants urgent messaging.
const SIGNIFICANT_STALENESS_THRESHOLD: usize = 10;

impl CodemapStaleness {
    /// Return true when this staleness level warrants `severity="high"` in the
    /// SessionStart injection. Callers use this to decide whether the injection
    /// should be *prepended* (so it lands in the visible preview) or appended.
    pub fn is_high_severity(&self, is_supervisor: bool) -> bool {
        match self {
            CodemapStaleness::Missing => true,
            CodemapStaleness::SignificantlyStale { .. } => true,
            CodemapStaleness::Stale { .. } => is_supervisor,
        }
    }

    /// Format as a context injection string for SessionStart.
    ///
    /// If `is_supervisor` is true, always uses strong language regardless of severity.
    pub fn format_injection(&self, is_supervisor: bool) -> String {
        match self {
            CodemapStaleness::Missing => {
                "<codemap-freshness severity=\"high\">\n\
                 CODEMAP.md is missing. Run `/codemap` before planning any work — \
                 agents waste significant tokens exploring without it.\n\
                 </codemap-freshness>"
                    .to_string()
            }
            CodemapStaleness::Stale {
                total_changes,
                file_list,
                commit_info,
            } => {
                let files = file_list.join(", ");
                let truncated = if *total_changes > 10 {
                    format!(" (+{} more)", total_changes - 10)
                } else {
                    String::new()
                };

                if is_supervisor {
                    format!(
                        "<codemap-freshness severity=\"high\">\n\
                         CODEMAP.md has {total_changes} structural change(s){commit_info} since last update: \
                         {files}{truncated}. Run `/codemap` to update before assigning work.\n\
                         </codemap-freshness>"
                    )
                } else {
                    format!(
                        "<codemap-freshness severity=\"info\">\n\
                         CODEMAP.md has {total_changes} pending structural change(s){commit_info}: \
                         {files}{truncated}.\n\
                         </codemap-freshness>"
                    )
                }
            }
            CodemapStaleness::SignificantlyStale {
                total_changes,
                file_list,
                commit_info,
            } => {
                let files = file_list.join(", ");
                let truncated = if *total_changes > 10 {
                    format!(" (+{} more)", total_changes - 10)
                } else {
                    String::new()
                };

                format!(
                    "<codemap-freshness severity=\"high\">\n\
                     CODEMAP.md is significantly out of date ({total_changes} structural changes{commit_info}): \
                     {files}{truncated}. Run `/codemap` to update before assigning work.\n\
                     </codemap-freshness>"
                )
            }
        }
    }
}

/// Check for codemap freshness and return staleness info.
///
/// Called from SessionStart and `cas codemap status` to determine whether
/// CODEMAP.md needs regeneration.
///
/// **Single source of truth (Strategy A):** uses git-based staleness exclusively.
/// The pending-changes ledger (.cas/codemap-pending.json) is intentionally NOT
/// consulted here — it only tracks committed changes (same signal as git) and
/// was the source of divergence between this function and `cas codemap status`.
/// The ledger remains available for `cas codemap pending` display; it is no
/// longer authoritative for freshness decisions.
///
/// Returns None if no action needed (codemap is up to date or fresh enough).
pub fn check_codemap_freshness(cas_root: &Path) -> Option<CodemapStaleness> {
    let project_root = cas_root.parent()?;

    let codemap_path = project_root.join(".claude/CODEMAP.md");

    if !codemap_path.exists() {
        return Some(CodemapStaleness::Missing);
    }

    // Sole mechanism: git-based staleness detection.
    // Works for terminal commits, worker commits, cherry-picks — any commit source.
    // After `/codemap` regen + commit, CODEMAP.md's git timestamp advances past
    // all prior structural changes → this returns None (up to date) automatically,
    // with no manual `cas codemap clear` step required.
    check_staleness_from_git(project_root)
}

/// Check staleness by comparing CODEMAP.md's last commit timestamp against
/// subsequent structural changes (A/D/R) in git history.
///
/// This is the primary detection mechanism — it works regardless of how commits
/// were made (terminal, worker, cherry-pick, etc.).
fn check_staleness_from_git(project_root: &Path) -> Option<CodemapStaleness> {
    // Get the timestamp of the last commit that touched CODEMAP.md
    let codemap_timestamp = get_codemap_last_commit_timestamp(project_root)?;

    // Find structural changes (A/D/R) since that timestamp
    let changes = get_structural_changes_since(project_root, &codemap_timestamp)?;

    if changes.is_empty() {
        return None;
    }

    let total_changes = changes.len();
    let file_list: Vec<String> = changes
        .iter()
        .take(10)
        .map(|c| {
            let prefix = match c.change_type.as_str() {
                "A" => "+",
                "D" => "-",
                "R" => "~",
                _ => "?",
            };
            format!("{prefix}{}", c.path)
        })
        .collect();

    Some(make_staleness(total_changes, file_list, String::new()))
}

/// Get the ISO timestamp of the last commit that modified .claude/CODEMAP.md.
///
/// Returns None if CODEMAP.md has never been committed or git fails.
fn get_codemap_last_commit_timestamp(project_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%cI", "--", ".claude/CODEMAP.md"])
        .current_dir(project_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let timestamp = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if timestamp.is_empty() {
        // CODEMAP.md exists on disk but was never committed — treat as stale
        // Fall back to file mtime
        return get_codemap_mtime_timestamp(project_root);
    }

    Some(timestamp)
}

/// Fallback: get CODEMAP.md's file modification time as an ISO timestamp.
fn get_codemap_mtime_timestamp(project_root: &Path) -> Option<String> {
    let codemap_path = project_root.join(".claude/CODEMAP.md");
    let metadata = std::fs::metadata(&codemap_path).ok()?;
    let mtime = metadata.modified().ok()?;
    let datetime: chrono::DateTime<chrono::Utc> = mtime.into();
    Some(datetime.to_rfc3339())
}

/// Find structural file changes (A/D/R) in git history since the given timestamp.
///
/// Uses `git log --diff-filter=ADR --name-status --since=<timestamp>` bounded
/// by the timestamp for performance (<500ms).
fn get_structural_changes_since(
    project_root: &Path,
    since: &str,
) -> Option<Vec<CodemapChange>> {
    let output = std::process::Command::new("git")
        .args([
            "log",
            "--diff-filter=ADR",
            "--name-status",
            &format!("--since={since}"),
            "--format=",
            "--no-renames",       // show as A+D instead of R for simplicity
            "-z",                 // NUL-delimited for safety with special chars
        ])
        .current_dir(project_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    parse_git_log_nul_output(&raw)
}

/// Parse NUL-delimited `git log --name-status -z` output.
///
/// Format: status\0path\0 (repeated), with empty lines between commits.
/// Only keeps A (added) and D (deleted) entries. Renames show as A+D
/// because we pass --no-renames.
fn parse_git_log_nul_output(raw: &str) -> Option<Vec<CodemapChange>> {
    let mut changes = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Split on NUL; entries come in pairs: status, path
    let parts: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;

    while i < parts.len() {
        let status = parts[i].trim_matches(|c: char| c == '\n' || c == '\r');
        if status.is_empty() {
            i += 1;
            continue;
        }

        // We need at least one more part for the path
        if i + 1 >= parts.len() {
            break;
        }

        let path = parts[i + 1].trim();
        if path.is_empty() {
            i += 2;
            continue;
        }

        // Deduplicate (same file may appear in multiple commits)
        let key = format!("{status}:{path}");
        if !seen.contains(&key) {
            seen.insert(key);
            match status {
                "A" => {
                    changes.push(CodemapChange {
                        change_type: "A".to_string(),
                        path: path.to_string(),
                        old_path: None,
                    });
                }
                "D" => {
                    changes.push(CodemapChange {
                        change_type: "D".to_string(),
                        path: path.to_string(),
                        old_path: None,
                    });
                }
                s if s.starts_with('R') => {
                    changes.push(CodemapChange {
                        change_type: "R".to_string(),
                        path: path.to_string(),
                        old_path: None,
                    });
                }
                _ => {}
            }
        }

        i += 2;
    }

    Some(changes)
}

/// Create the appropriate staleness variant based on change count.
fn make_staleness(total_changes: usize, file_list: Vec<String>, commit_info: String) -> CodemapStaleness {
    if total_changes >= SIGNIFICANT_STALENESS_THRESHOLD {
        CodemapStaleness::SignificantlyStale {
            total_changes,
            file_list,
            commit_info,
        }
    } else {
        CodemapStaleness::Stale {
            total_changes,
            file_list,
            commit_info,
        }
    }
}

/// Best-effort codemap reminder for Stop hook.
///
/// Returns a reminder string if there are pending structural changes.
/// Checks both pending file and git history.
pub fn codemap_stop_reminder(cas_root: &Path) -> Option<String> {
    let pending_path = cas_root.join(CODEMAP_PENDING_FILE);

    // Check pending file first (don't use ? here — read failure must not skip git fallback)
    if pending_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&pending_path) {
            let total: usize = content
                .lines()
                .filter_map(|line| serde_json::from_str::<CodemapPending>(line.trim()).ok())
                .map(|p| p.changes.len())
                .sum();

            if total > 0 {
                return Some(format!(
                    "Note: CODEMAP.md has {total} pending structural change(s) that should be updated."
                ));
            }
        }
    }

    // Fall back to git-based check
    let project_root = cas_root.parent()?;
    let codemap_path = project_root.join(".claude/CODEMAP.md");
    if !codemap_path.exists() {
        return None;
    }

    let timestamp = get_codemap_last_commit_timestamp(project_root)?;
    let changes = get_structural_changes_since(project_root, &timestamp)?;
    if changes.is_empty() {
        return None;
    }

    Some(format!(
        "Note: CODEMAP.md has {} pending structural change(s) that should be updated.",
        changes.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_diff_tree_added() {
        let output = "A\tsrc/new_module.rs\n";
        let changes = parse_diff_tree_output(output).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, "A");
        assert_eq!(changes[0].path, "src/new_module.rs");
        assert!(changes[0].old_path.is_none());
    }

    #[test]
    fn test_parse_diff_tree_deleted() {
        let output = "D\tsrc/old_module.rs\n";
        let changes = parse_diff_tree_output(output).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, "D");
        assert_eq!(changes[0].path, "src/old_module.rs");
    }

    #[test]
    fn test_parse_diff_tree_renamed() {
        let output = "R100\tsrc/old_name.rs\tsrc/new_name.rs\n";
        let changes = parse_diff_tree_output(output).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, "R");
        assert_eq!(changes[0].path, "src/new_name.rs");
        assert_eq!(changes[0].old_path.as_deref(), Some("src/old_name.rs"));
    }

    #[test]
    fn test_parse_diff_tree_ignores_modified() {
        let output = "M\tsrc/existing.rs\nA\tsrc/new.rs\n";
        let changes = parse_diff_tree_output(output).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, "A");
    }

    #[test]
    fn test_parse_diff_tree_mixed() {
        let output = "A\tsrc/new.rs\nD\tsrc/old.rs\nM\tsrc/modified.rs\nR095\tsrc/a.rs\tsrc/b.rs\n";
        let changes = parse_diff_tree_output(output).unwrap();
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].change_type, "A");
        assert_eq!(changes[1].change_type, "D");
        assert_eq!(changes[2].change_type, "R");
    }

    #[test]
    fn test_parse_diff_tree_empty() {
        let changes = parse_diff_tree_output("").unwrap();
        assert!(changes.is_empty());
    }

    #[test]
    fn test_codemap_pending_serialization() {
        let pending = CodemapPending {
            changes: vec![
                CodemapChange {
                    change_type: "A".to_string(),
                    path: "src/new.rs".to_string(),
                    old_path: None,
                },
                CodemapChange {
                    change_type: "D".to_string(),
                    path: "src/old.rs".to_string(),
                    old_path: None,
                },
            ],
            commit: "abc1234".to_string(),
            recorded_at: "2026-04-03T18:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&pending).unwrap();
        let deserialized: CodemapPending = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.changes.len(), 2);
        assert_eq!(deserialized.commit, "abc1234");
    }

    #[test]
    fn test_codemap_pending_jsonl_parsing() {
        let jsonl = r#"{"changes":[{"type":"A","path":"src/new.rs"}],"commit":"abc1234","recorded_at":"2026-04-03T18:00:00Z"}
{"changes":[{"type":"D","path":"src/old.rs"}],"commit":"def5678","recorded_at":"2026-04-03T19:00:00Z"}"#;

        let entries: Vec<CodemapPending> = jsonl
            .lines()
            .filter_map(|line| serde_json::from_str(line.trim()).ok())
            .collect();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].commit, "abc1234");
        assert_eq!(entries[1].commit, "def5678");
    }

    #[test]
    fn test_parse_git_log_nul_output_added_and_deleted() {
        // Simulates: git log --diff-filter=ADR --name-status -z --format= output
        // NUL-separated: status\0path\0
        let raw = "A\0src/new_file.rs\0D\0src/removed.rs\0";
        let changes = parse_git_log_nul_output(raw).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].change_type, "A");
        assert_eq!(changes[0].path, "src/new_file.rs");
        assert_eq!(changes[1].change_type, "D");
        assert_eq!(changes[1].path, "src/removed.rs");
    }

    #[test]
    fn test_parse_git_log_nul_output_deduplicates() {
        // Same file added in two commits should appear once
        let raw = "A\0src/file.rs\0A\0src/file.rs\0";
        let changes = parse_git_log_nul_output(raw).unwrap();
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn test_parse_git_log_nul_output_empty() {
        let changes = parse_git_log_nul_output("").unwrap();
        assert!(changes.is_empty());
    }

    #[test]
    fn test_parse_git_log_nul_output_ignores_modified() {
        let raw = "M\0src/modified.rs\0A\0src/new.rs\0";
        let changes = parse_git_log_nul_output(raw).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, "A");
    }

    #[test]
    fn test_parse_git_log_nul_output_with_newlines_between_commits() {
        // git log output may have newlines between commit boundaries
        let raw = "A\0src/a.rs\0\nD\0src/b.rs\0";
        let changes = parse_git_log_nul_output(raw).unwrap();
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn test_is_high_severity_missing_always_high() {
        assert!(CodemapStaleness::Missing.is_high_severity(false));
        assert!(CodemapStaleness::Missing.is_high_severity(true));
    }

    #[test]
    fn test_is_high_severity_significantly_stale_always_high() {
        let staleness = CodemapStaleness::SignificantlyStale {
            total_changes: 20,
            file_list: vec![],
            commit_info: String::new(),
        };
        assert!(staleness.is_high_severity(false));
        assert!(staleness.is_high_severity(true));
    }

    #[test]
    fn test_is_high_severity_stale_supervisor_only() {
        let staleness = CodemapStaleness::Stale {
            total_changes: 3,
            file_list: vec![],
            commit_info: String::new(),
        };
        assert!(!staleness.is_high_severity(false));
        assert!(staleness.is_high_severity(true));
    }

    #[test]
    fn test_make_staleness_below_threshold() {
        let staleness = make_staleness(
            3,
            vec!["+a.rs".to_string()],
            String::new(),
        );
        assert!(matches!(staleness, CodemapStaleness::Stale { .. }));
    }

    #[test]
    fn test_make_staleness_at_threshold() {
        let staleness = make_staleness(
            10,
            vec!["+a.rs".to_string()],
            String::new(),
        );
        assert!(matches!(staleness, CodemapStaleness::SignificantlyStale { .. }));
    }

    #[test]
    fn test_missing_injection_is_urgent() {
        let msg = CodemapStaleness::Missing.format_injection(false);
        assert!(msg.contains("severity=\"high\""));
        assert!(msg.contains("CODEMAP.md is missing"));
        assert!(msg.contains("/codemap"));
    }

    #[test]
    fn test_stale_injection_info_for_regular_session() {
        let staleness = CodemapStaleness::Stale {
            total_changes: 3,
            file_list: vec!["+src/a.rs".to_string(), "-src/b.rs".to_string()],
            commit_info: String::new(),
        };
        let msg = staleness.format_injection(false);
        assert!(msg.contains("severity=\"info\""));
        assert!(msg.contains("3 pending structural change(s)"));
        assert!(!msg.contains("Run `/codemap`"));
    }

    #[test]
    fn test_stale_injection_urgent_for_supervisor() {
        let staleness = CodemapStaleness::Stale {
            total_changes: 3,
            file_list: vec!["+src/a.rs".to_string()],
            commit_info: String::new(),
        };
        let msg = staleness.format_injection(true);
        assert!(msg.contains("severity=\"high\""));
        assert!(msg.contains("Run `/codemap`"));
    }

    #[test]
    fn test_significantly_stale_injection_always_urgent() {
        let staleness = CodemapStaleness::SignificantlyStale {
            total_changes: 15,
            file_list: (0..10).map(|i| format!("+src/file{i}.rs")).collect(),
            commit_info: " since abc1234".to_string(),
        };
        // Urgent even for non-supervisor
        let msg = staleness.format_injection(false);
        assert!(msg.contains("severity=\"high\""));
        assert!(msg.contains("significantly out of date"));
        assert!(msg.contains("15 structural changes"));
        assert!(msg.contains("(+5 more)"));
        assert!(msg.contains("Run `/codemap`"));
    }

    #[test]
    fn test_check_codemap_freshness_no_cas_parent() {
        // cas_root with no parent should return None
        let result = check_codemap_freshness(Path::new("/"));
        // Root has no parent, returns None
        assert!(result.is_none());
    }

    /// Strategy A invariant: pending-ledger entries alone do NOT mark the codemap stale.
    /// Only git-based staleness drives the freshness signal.
    ///
    /// Before the Strategy A fix, check_codemap_freshness() would return Some(Stale)
    /// when the pending file had entries, even after CODEMAP.md was regenerated and
    /// committed. Now it ignores the pending file entirely.
    ///
    /// In this test we can't replicate a real git repo, but we CAN verify that:
    /// - a pending file with entries does NOT cause the function to return Some
    /// - the function falls through to git (which fails in a temp dir → returns None)
    #[test]
    fn test_codemap_freshness_pending_file_does_not_override_git() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        // Lay out: dir.path()/.cas/  →  cas_root
        //          dir.path()/       →  project_root
        let cas_root = dir.path().join(".cas");
        let project_root = dir.path();

        fs::create_dir_all(&cas_root).unwrap();

        // Create CODEMAP.md so freshness check doesn't short-circuit to Missing.
        let claude_dir = project_root.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(claude_dir.join("CODEMAP.md"), "# Codemap\n").unwrap();

        // Populate the pending ledger with entries (simulates the pre-fix divergence state).
        let pending_json = r#"{"changes":[{"type":"A","path":"src/new.rs"},{"type":"D","path":"src/old.rs"}],"commit":"abc1234","recorded_at":"2026-04-03T18:00:00Z"}"#;
        fs::write(cas_root.join(CODEMAP_PENDING_FILE), pending_json).unwrap();

        // With Strategy A: pending file is NOT consulted; git is the authority.
        // There is no git repo here, so git returns no timestamp → check_staleness_from_git
        // returns None → check_codemap_freshness returns None (up to date).
        let result = check_codemap_freshness(&cas_root);
        assert!(
            result.is_none(),
            "pending-ledger entries must not mark codemap stale when git shows up to date"
        );
    }

    #[test]
    fn test_codemap_stop_reminder_no_file() {
        let temp = std::env::temp_dir().join("test_codemap_stop_reminder");
        let _ = std::fs::create_dir_all(&temp);
        let result = codemap_stop_reminder(&temp);
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_codemap_stop_reminder_with_pending() {
        let temp = std::env::temp_dir().join("test_codemap_stop_with_pending");
        let _ = std::fs::create_dir_all(&temp);

        let pending_path = temp.join(CODEMAP_PENDING_FILE);
        std::fs::write(
            &pending_path,
            r#"{"changes":[{"type":"A","path":"src/new.rs"},{"type":"D","path":"src/old.rs"}],"commit":"abc1234","recorded_at":"2026-04-03T18:00:00Z"}"#,
        )
        .unwrap();

        let result = codemap_stop_reminder(&temp);
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("2 pending structural change(s)"));

        let _ = std::fs::remove_dir_all(&temp);
    }
}
