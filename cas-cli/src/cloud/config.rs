//! Cloud configuration management
//!
//! Stores cloud authentication and sync state in `.cas/cloud.json`.
//!
//! # Integration Status
//! Methods ready for cloud sync feature when enabled.

// #![allow(dead_code)] // Check unused

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::CasError;
use crate::store::find_cas_root;

/// Cached project canonical ID. Only `Some` results are cached; if resolution
/// returns `None` (e.g. `find_cas_root()` fails because the process started
/// outside a CAS project), the next call retries instead of locking in `None`
/// for the process lifetime. This prevents transient failures during daemon
/// startup or early session hooks from permanently disabling project scoping.
static CACHED_PROJECT_ID: Mutex<Option<String>> = Mutex::new(None);

/// Get the canonical project ID for the current CAS project.
///
/// The canonical ID is the folder name of the project root directory (the directory
/// containing `.cas/`). This is:
/// - Stable across git remote changes (fork, transfer, rename)
/// - Works for non-git projects
/// - Human-readable in logs, UI, and team project lists
///
/// Examples:
/// - `/home/user/projects/petra-stella-cloud/.cas/` → `petra-stella-cloud`
/// - `/home/user/cas-src/.cas/` → `cas-src`
/// - `/home/user/gabber-studio/.cas/` → `gabber-studio`
///
/// If the folder name cannot be derived (e.g. `.cas/` lives at the filesystem root
/// and its parent has no file name), falls back to a deterministic `local:<sha256>`
/// hash of the canonicalized project path. This guarantees every valid CAS project
/// has a stable, unique `project_id` for cloud sync scoping.
///
/// Returns `None` only if not inside a CAS project directory at all.
/// Successful results are cached for the process lifetime; `None` results
/// are retried on each call so transient failures don't stick.
pub fn get_project_canonical_id() -> Option<String> {
    let mut cached = CACHED_PROJECT_ID.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref id) = *cached {
        return Some(id.clone());
    }
    // Not yet resolved — try now. Only cache Some results.
    let result = find_cas_root().ok().and_then(|root| resolve_canonical_id(&root));
    if result.is_some() {
        *cached = result.clone();
    }
    result
}

/// Pure composition of the canonical-id resolution chain.
/// Extracted from `get_project_canonical_id` so the chain is testable
/// without the `OnceLock` static — callers should prefer the cached public API.
///
/// Resolution order (highest priority first):
///  1. `.cas/config.toml [project] canonical_id` — explicit source of truth,
///     set eagerly by `cas cloud team set` or manually via
///     `cas cloud project set` (cas-1ced).
///  2. Parent-directory folder name — legacy default that ships before
///     team_set lands a config-toml entry.
///  3. Path-hash fallback — for the `.cas/` at filesystem root edge case.
pub fn resolve_canonical_id(cas_root: &Path) -> Option<String> {
    canonical_id_from_config_toml(cas_root)
        .or_else(|| canonical_id_from_cas_root(cas_root))
        .or_else(|| fallback_project_id_from_path(cas_root))
}

/// Read `[project] canonical_id` from `<cas_root>/config.toml`. Returns
/// `None` when the file is missing, parse fails, the `[project]` block is
/// absent, or `canonical_id` is unset. This is a best-effort read — any
/// failure falls through to the next resolution step.
pub fn canonical_id_from_config_toml(cas_root: &Path) -> Option<String> {
    let toml_path = cas_root.join("config.toml");
    let content = std::fs::read_to_string(&toml_path).ok()?;
    let parsed: toml::Value = toml::from_str(&content).ok()?;
    parsed
        .get("project")?
        .get("canonical_id")?
        .as_str()
        .map(|s| s.to_string())
}

/// Write `[project] canonical_id = "<value>"` to `<cas_root>/config.toml`,
/// preserving any other existing sections. Read-modify-write via the `toml`
/// crate so prior `[memory]`, `[code_review]`, etc. blocks survive.
///
/// Returns `Err` only on IO or TOML serialization failure. Callers should
/// surface the error — the value did NOT land if this fails.
pub fn set_canonical_id_in_config_toml(
    cas_root: &Path,
    canonical_id: &str,
) -> Result<(), CasError> {
    let toml_path = cas_root.join("config.toml");

    // Read-modify-write: parse existing content (or start with empty table
    // if absent), update [project].canonical_id, serialize back.
    let mut doc: toml::Value = match std::fs::read_to_string(&toml_path) {
        Ok(content) => toml::from_str(&content)
            .map_err(|e| CasError::Other(format!("Failed to parse config.toml: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Value::Table(toml::value::Table::new()),
        Err(e) => return Err(CasError::Other(format!("Failed to read config.toml: {e}"))),
    };

    let table = doc
        .as_table_mut()
        .ok_or_else(|| CasError::Other("config.toml root is not a table".to_string()))?;

    let project = table
        .entry("project".to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
        .as_table_mut()
        .ok_or_else(|| CasError::Other("config.toml [project] is not a table".to_string()))?;
    project.insert(
        "canonical_id".to_string(),
        toml::Value::String(canonical_id.to_string()),
    );

    let serialized = toml::to_string_pretty(&doc)
        .map_err(|e| CasError::Other(format!("Failed to serialize config.toml: {e}")))?;

    // Ensure cas_root exists before writing.
    if let Some(parent) = toml_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CasError::Other(format!("Failed to create {parent:?}: {e}")))?;
    }
    std::fs::write(&toml_path, serialized)
        .map_err(|e| CasError::Other(format!("Failed to write config.toml: {e}")))?;
    Ok(())
}

/// Derive the canonical project ID from `git -C <cas_root> remote get-url origin`,
/// normalized to `<host>/<owner>/<repo>` form (strips `https?://` / `git@HOST:`
/// prefix and `.git` suffix). Returns `None` when:
///  - git binary isn't available
///  - cas_root isn't a git repo (or has no `origin` remote)
///  - the URL doesn't match a recognizable form
///
/// Used by `cas cloud team set` (cas-1ced) as the second resolution step
/// after `.cas/config.toml`. Never invoked by the cached production
/// `get_project_canonical_id` chain — only by the eager `team set` flow.
pub fn derive_canonical_id_from_git_remote(cas_root: &Path) -> Option<String> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["-C"])
        .arg(cas_root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    normalize_git_remote_url(raw.trim())
}

/// Normalize a git remote URL to `<host>/<owner>/<repo>` form.
///
/// Recognized inputs:
///  - `https://host/owner/repo[.git]` → `host/owner/repo`
///  - `http://host/owner/repo[.git]` → `host/owner/repo`
///  - `ssh://git@host/owner/repo[.git]` → `host/owner/repo`
///  - `git@host:owner/repo[.git]` → `host/owner/repo`
///
/// Returns `None` for anything else (e.g. local file paths, malformed
/// URLs) so the caller can fall through to the next resolution step
/// rather than persist a non-canonical value.
pub fn normalize_git_remote_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    // SSH form: `git@host:owner/repo[.git]`. Replace the `:` with `/` after
    // stripping the user prefix so the parse falls through to the generic
    // `host/owner/repo` extractor below.
    let without_ssh_user = if let Some(rest) = trimmed.strip_prefix("git@") {
        // Find the first `:` — that's the separator between host and path.
        let (host, path) = rest.split_once(':')?;
        format!("{host}/{path}")
    } else if let Some(rest) = trimmed.strip_prefix("ssh://git@") {
        // ssh://git@host/path → strip prefix; rest already uses `/`.
        rest.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        rest.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        rest.to_string()
    } else {
        return None;
    };

    // Strip optional `.git` suffix.
    let without_dot_git = without_ssh_user
        .strip_suffix(".git")
        .unwrap_or(&without_ssh_user);
    // Strip optional trailing slash for paranoia.
    let clean = without_dot_git.trim_end_matches('/');

    if clean.is_empty() {
        None
    } else {
        Some(clean.to_string())
    }
}

/// Derive the canonical project ID from a `.cas` directory path.
///
/// The canonical ID is the folder name of the parent directory (the project root).
/// Returns `None` if the path has no parent or no file name (e.g. filesystem root).
pub fn canonical_id_from_cas_root(cas_root: &Path) -> Option<String> {
    let project_dir = cas_root.parent().unwrap_or(cas_root);
    project_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

/// Fallback project ID derived from a deterministic sha256 hash of the canonical
/// project path. Used when `canonical_id_from_cas_root` cannot produce a folder
/// name (e.g. `.cas/` at the filesystem root).
///
/// Format: `local:<first 16 hex chars of sha256(canonical_path)>` — 8 bytes of
/// entropy, more than enough to avoid collisions on a single machine while staying
/// compact in URLs and logs.
///
/// The input is the parent of `cas_root` (the project directory), canonicalized
/// via `std::fs::canonicalize` when possible so symlinked and renamed paths
/// produce the same ID. Falls back to the lexical path if canonicalization fails
/// (e.g. the directory no longer exists on disk — should not happen in practice
/// since we just resolved it via `find_cas_root`, but we stay defensive).
///
/// Returns `None` only if both the canonical and lexical paths fail to produce
/// any bytes to hash — practically unreachable.
pub fn fallback_project_id_from_path(cas_root: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};

    let project_dir = cas_root.parent().unwrap_or(cas_root);
    let canonical = std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let path_bytes = canonical.as_os_str().as_encoded_bytes();
    if path_bytes.is_empty() {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(path_bytes);
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    Some(format!("local:{hex}"))
}

/// Cloud configuration stored in .cas/cloud.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Cloud API endpoint
    #[serde(default = "default_endpoint")]
    pub endpoint: String,

    /// API token for authentication
    pub token: Option<String>,

    /// User email
    pub email: Option<String>,

    /// User plan
    pub plan: Option<String>,

    /// Organization ID (for enterprise users)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,

    /// Organization slug (for display)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_slug: Option<String>,

    /// Team ID (for enterprise users)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    /// Team slug (for display)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_slug: Option<String>,

    /// Per-team sync timestamps (team_id -> last sync time)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub team_sync_timestamps: HashMap<String, DateTime<Utc>>,

    /// Per-project team memory sync timestamps (canonical_id -> last pull time)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub team_memory_sync_timestamps: HashMap<String, String>,

    /// Last sync timestamp for entries
    pub last_entry_sync: Option<String>,

    /// Last sync timestamp for tasks
    pub last_task_sync: Option<String>,

    /// Last sync timestamp for rules
    pub last_rule_sync: Option<String>,

    /// Last sync timestamp for skills
    pub last_skill_sync: Option<String>,

    /// Whether the factory daemon should spawn its live-stream WebSocket
    /// client (phone-home / relay / pane-watch).
    ///
    /// Default: `false`. The client targets a Phoenix-framework WebSocket
    /// endpoint (`/socket/websocket`) that the current Next.js cloud backend
    /// does not implement and cannot host on Vercel. Leaving the client off
    /// by default avoids the 10-retry 404 storm (~4 min of log noise per
    /// factory session) that cas-4244 documented.
    ///
    /// Re-enable by setting this field to `true` in `.cas/cloud.json` once a
    /// Phoenix-capable backend is reachable (e.g. when the Hetzner Slack
    /// bridge is re-deployed — see `project_claude_code_account_banned`).
    /// The REST-based cloud syncer (`cas-cli/src/cloud/syncer/`) is
    /// independent of this flag and always runs when logged in.
    #[serde(default)]
    pub factory_cloud_client_enabled: bool,

    /// Whether automatic team auto-promotion is enabled for this folder.
    ///
    /// When `None` or `Some(true)` (the default), the syncing store
    /// wrappers dual-enqueue eligible writes to the team queue whenever
    /// `team_id` is set. `Some(false)` disables the coarse trigger — only
    /// explicit `cas memory remember --share team` / `cas memory share`
    /// primitives (T5) push to the team queue after that.
    ///
    /// See `docs/requests/team-memories-filter-policy.md` Decision 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_auto_promote: Option<bool>,
}

fn default_endpoint() -> String {
    "https://cas.dev".to_string()
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            token: None,
            email: None,
            plan: None,
            org_id: None,
            org_slug: None,
            team_id: None,
            team_slug: None,
            team_sync_timestamps: HashMap::new(),
            team_memory_sync_timestamps: HashMap::new(),
            last_entry_sync: None,
            last_task_sync: None,
            last_rule_sync: None,
            last_skill_sync: None,
            factory_cloud_client_enabled: false,
            team_auto_promote: None,
        }
    }
}

impl CloudConfig {
    /// Load cloud config from .cas/cloud.json
    pub fn load() -> Result<Self, CasError> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    /// Load cloud config from a specific path
    pub fn load_from(path: &Path) -> Result<Self, CasError> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let config: Self = serde_json::from_str(&content)
                .map_err(|e| CasError::Other(format!("Failed to parse cloud config: {e}")))?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Load cloud config from a specific cas directory
    pub fn load_from_cas_dir(cas_dir: &Path) -> Result<Self, CasError> {
        let path = cas_dir.join("cloud.json");
        Self::load_from(&path)
    }

    /// Save cloud config to .cas/cloud.json
    pub fn save(&self) -> Result<(), CasError> {
        let path = Self::config_path()?;
        self.save_to(&path)
    }

    /// Save cloud config to a specific path
    pub fn save_to(&self, path: &Path) -> Result<(), CasError> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| CasError::Other(format!("Failed to serialize cloud config: {e}")))?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Save cloud config to a specific cas directory
    pub fn save_to_cas_dir(&self, cas_dir: &Path) -> Result<(), CasError> {
        let path = cas_dir.join("cloud.json");
        self.save_to(&path)
    }

    /// Get the path to cloud.json
    pub fn config_path() -> Result<PathBuf, CasError> {
        let cas_root = find_cas_root()?;
        Ok(cas_root.join("cloud.json"))
    }

    /// Check if user is logged in (has a valid token)
    pub fn is_logged_in(&self) -> bool {
        self.token.as_ref().is_some_and(|t| !t.is_empty())
    }

    /// Clear authentication (logout)
    pub fn logout(&mut self) {
        self.token = None;
        self.email = None;
        self.plan = None;
        self.org_id = None;
        self.org_slug = None;
        self.team_id = None;
        self.team_slug = None;
    }

    /// Check if user belongs to an organization
    pub fn has_org(&self) -> bool {
        self.org_id.is_some()
    }

    /// Check if user belongs to a team
    pub fn has_team(&self) -> bool {
        self.team_id.is_some()
    }

    /// Return the team UUID to auto-promote writes to, or `None` if team
    /// auto-promotion is disabled for this folder.
    ///
    /// Distinct from `team_id` directly: this accessor honours the
    /// `team_auto_promote` coarse kill-switch. `Some(false)` on
    /// `team_auto_promote` returns `None` here even if `team_id` is set —
    /// the user has opted out of automatic dual-enqueue. Callers building
    /// the T1 filter predicate should use this accessor, not `team_id`.
    pub fn active_team_id(&self) -> Option<&str> {
        if matches!(self.team_auto_promote, Some(false)) {
            return None;
        }
        self.team_id.as_deref()
    }

    /// Set the current team context
    pub fn set_team(&mut self, team_id: &str, team_slug: &str) {
        self.team_id = Some(team_id.to_string());
        self.team_slug = Some(team_slug.to_string());
    }

    /// Clear the current team context
    pub fn clear_team(&mut self) {
        self.team_id = None;
        self.team_slug = None;
    }

    /// Get the last sync timestamp for a specific team
    pub fn get_team_sync_timestamp(&self, team_id: &str) -> Option<DateTime<Utc>> {
        self.team_sync_timestamps.get(team_id).copied()
    }

    /// Set the last sync timestamp for a specific team
    pub fn set_team_sync_timestamp(&mut self, team_id: &str, ts: DateTime<Utc>) {
        self.team_sync_timestamps.insert(team_id.to_string(), ts);
    }

    /// Clear the sync timestamp for a specific team
    pub fn clear_team_sync_timestamp(&mut self, team_id: &str) {
        self.team_sync_timestamps.remove(team_id);
    }

    /// Get the last team memory sync timestamp for a project
    pub fn get_team_memory_sync(&self, canonical_id: &str) -> Option<&str> {
        self.team_memory_sync_timestamps
            .get(canonical_id)
            .map(|s| s.as_str())
    }

    /// Set the last team memory sync timestamp for a project
    pub fn set_team_memory_sync(&mut self, canonical_id: &str, timestamp: &str) {
        self.team_memory_sync_timestamps
            .insert(canonical_id.to_string(), timestamp.to_string());
    }
}

#[cfg(test)]
mod tests {
    use crate::cloud::config::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = CloudConfig::default();
        assert_eq!(config.endpoint, "https://cas.dev");
        assert!(config.token.is_none());
        assert!(!config.is_logged_in());
    }

    #[test]
    fn test_save_and_load() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cloud.json");

        let config = CloudConfig {
            token: Some("test_token".to_string()),
            email: Some("test@example.com".to_string()),
            ..Default::default()
        };

        config.save_to(&path).unwrap();

        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(loaded.token, Some("test_token".to_string()));
        assert_eq!(loaded.email, Some("test@example.com".to_string()));
        assert!(loaded.is_logged_in());
    }

    #[test]
    fn test_logout() {
        let mut config = CloudConfig {
            token: Some("test_token".to_string()),
            email: Some("test@example.com".to_string()),
            ..Default::default()
        };

        assert!(config.is_logged_in());

        config.logout();

        assert!(!config.is_logged_in());
        assert!(config.token.is_none());
        assert!(config.email.is_none());
    }

    #[test]
    fn test_set_and_clear_team() {
        let mut config = CloudConfig::default();
        assert!(!config.has_team());
        assert!(config.team_id.is_none());
        assert!(config.team_slug.is_none());

        config.set_team("team-123", "my-team");
        assert!(config.has_team());
        assert_eq!(config.team_id, Some("team-123".to_string()));
        assert_eq!(config.team_slug, Some("my-team".to_string()));

        config.clear_team();
        assert!(!config.has_team());
        assert!(config.team_id.is_none());
        assert!(config.team_slug.is_none());
    }

    #[test]
    fn test_active_team_id_returns_none_when_no_team_set() {
        let config = CloudConfig::default();
        assert_eq!(config.active_team_id(), None);
    }

    #[test]
    fn test_active_team_id_returns_team_when_auto_promote_is_default() {
        // team_auto_promote=None is the default — auto-promote enabled.
        let mut config = CloudConfig::default();
        config.set_team("team-abc", "my-team");
        assert_eq!(config.active_team_id(), Some("team-abc"));
        assert!(config.team_auto_promote.is_none());
    }

    #[test]
    fn test_active_team_id_returns_team_when_auto_promote_is_true() {
        let mut config = CloudConfig::default();
        config.set_team("team-abc", "my-team");
        config.team_auto_promote = Some(true);
        assert_eq!(config.active_team_id(), Some("team-abc"));
    }

    #[test]
    fn test_active_team_id_suppressed_by_auto_promote_false() {
        // The coarse kill-switch from Decision 3 of filter-policy.md —
        // team_id still set, but dual-enqueue is disabled.
        let mut config = CloudConfig::default();
        config.set_team("team-abc", "my-team");
        config.team_auto_promote = Some(false);
        assert_eq!(config.active_team_id(), None);
    }

    #[test]
    fn test_team_sync_timestamps() {
        let mut config = CloudConfig::default();

        // Initially no timestamps
        assert!(config.get_team_sync_timestamp("team-a").is_none());

        // Set timestamp for team-a
        let ts1 = Utc::now();
        config.set_team_sync_timestamp("team-a", ts1);
        assert_eq!(config.get_team_sync_timestamp("team-a"), Some(ts1));

        // Set timestamp for team-b
        let ts2 = Utc::now();
        config.set_team_sync_timestamp("team-b", ts2);
        assert_eq!(config.get_team_sync_timestamp("team-b"), Some(ts2));

        // team-a still has its timestamp
        assert_eq!(config.get_team_sync_timestamp("team-a"), Some(ts1));

        // Clear team-a timestamp
        config.clear_team_sync_timestamp("team-a");
        assert!(config.get_team_sync_timestamp("team-a").is_none());
        assert_eq!(config.get_team_sync_timestamp("team-b"), Some(ts2));
    }

    #[test]
    fn test_team_memory_sync_timestamps() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cloud.json");

        let mut config = CloudConfig {
            token: Some("t".to_string()),
            ..Default::default()
        };

        // Initially no timestamp
        assert!(config.get_team_memory_sync("github.com/foo/bar").is_none());

        // Set and get
        config.set_team_memory_sync("github.com/foo/bar", "2026-04-02T10:00:00Z");
        assert_eq!(
            config.get_team_memory_sync("github.com/foo/bar"),
            Some("2026-04-02T10:00:00Z")
        );

        // Persists through save/load
        config.save_to(&path).unwrap();
        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(
            loaded.get_team_memory_sync("github.com/foo/bar"),
            Some("2026-04-02T10:00:00Z")
        );
    }

    #[test]
    fn test_canonical_id_from_cas_root() {
        // Create real temp directories simulating different project layouts
        let temp = TempDir::new().unwrap();

        // Simulate /tmp/.../petra-stella-cloud/.cas
        let project_a = temp.path().join("petra-stella-cloud");
        let cas_a = project_a.join(".cas");
        std::fs::create_dir_all(&cas_a).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_a),
            Some("petra-stella-cloud".to_string())
        );

        // Simulate /tmp/.../gabber-studio/.cas
        let project_b = temp.path().join("gabber-studio");
        let cas_b = project_b.join(".cas");
        std::fs::create_dir_all(&cas_b).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_b),
            Some("gabber-studio".to_string())
        );

        // Non-git project works the same way
        let project_c = temp.path().join("local-only-project");
        let cas_c = project_c.join(".cas");
        std::fs::create_dir_all(&cas_c).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_c),
            Some("local-only-project".to_string())
        );

        // Folder with spaces
        let project_d = temp.path().join("Richards LLC");
        let cas_d = project_d.join(".cas");
        std::fs::create_dir_all(&cas_d).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_d),
            Some("Richards LLC".to_string())
        );
    }

    #[test]
    fn test_canonical_id_from_filesystem_root() {
        // Edge case: .cas at filesystem root — parent is "/" which has no file_name
        use std::path::Path;
        let root_cas = Path::new("/.cas");
        assert_eq!(canonical_id_from_cas_root(root_cas), None);
    }

    #[test]
    fn test_fallback_project_id_from_path_is_deterministic() {
        // Same input path produces the same hash across repeated invocations,
        // and the format is `local:` + 16 lowercase-hex chars (8 bytes of sha256).
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("some-project");
        let cas_dir = project_dir.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();

        let first = fallback_project_id_from_path(&cas_dir).unwrap();
        let second = fallback_project_id_from_path(&cas_dir).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("local:"));
        // local: + 16 hex chars = 22 chars total
        assert_eq!(first.len(), 22);
        // Every char after the `local:` prefix must be a lowercase ASCII hex digit.
        let suffix = &first[6..];
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "fallback suffix should be lowercase hex, got {suffix:?}"
        );
    }

    #[test]
    fn test_fallback_project_id_from_path_is_unique_per_path() {
        // Different project paths must produce different hashes — otherwise two
        // projects at different locations would still collide.
        let temp = TempDir::new().unwrap();

        let project_a = temp.path().join("project-a");
        let cas_a = project_a.join(".cas");
        std::fs::create_dir_all(&cas_a).unwrap();

        let project_b = temp.path().join("project-b");
        let cas_b = project_b.join(".cas");
        std::fs::create_dir_all(&cas_b).unwrap();

        let id_a = fallback_project_id_from_path(&cas_a).unwrap();
        let id_b = fallback_project_id_from_path(&cas_b).unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_fallback_project_id_handles_filesystem_root() {
        // The whole point of the fallback: at filesystem root,
        // canonical_id_from_cas_root returns None; fallback must still produce a value.
        use std::path::Path;
        let root_cas = Path::new("/.cas");
        assert_eq!(canonical_id_from_cas_root(root_cas), None);

        let fallback = fallback_project_id_from_path(root_cas);
        assert!(fallback.is_some());
        let id = fallback.unwrap();
        assert!(id.starts_with("local:"));
        assert_eq!(id.len(), 22);
    }

    #[test]
    fn test_resolve_canonical_id_prefers_folder_name() {
        // End-to-end coverage of the .or_else chain: when the folder name is
        // available, resolve_canonical_id returns it unchanged — the fallback
        // must not fire on the happy path.
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("my-project");
        let cas_dir = project_dir.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();

        let id = resolve_canonical_id(&cas_dir).unwrap();
        assert_eq!(id, "my-project");
        assert!(!id.starts_with("local:"));
    }

    #[test]
    fn test_resolve_canonical_id_falls_back_at_filesystem_root() {
        // End-to-end: when folder name is unavailable (filesystem root),
        // resolve_canonical_id returns Some("local:...") instead of None.
        // A regression that dropped the `.or_else` would turn this back into None.
        use std::path::Path;
        let root_cas = Path::new("/.cas");
        let id = resolve_canonical_id(root_cas).expect("fallback should fire at fs root");
        assert!(id.starts_with("local:"));
        assert_eq!(id.len(), 22);
    }

    #[test]
    fn test_fallback_lexical_branch_when_canonicalize_fails() {
        // `fallback_project_id_from_path` falls back to the lexical path when
        // `std::fs::canonicalize` fails (e.g., the directory does not exist on
        // disk). Point it at a non-existent path and verify we still get a
        // stable `local:<hex>` value rather than a panic or None.
        let temp = TempDir::new().unwrap();
        let nonexistent_cas = temp.path().join("never-created").join(".cas");
        // Intentionally do NOT create the directory.

        let id = fallback_project_id_from_path(&nonexistent_cas)
            .expect("fallback must tolerate non-canonicalizable paths");
        assert!(id.starts_with("local:"));
        assert_eq!(id.len(), 22);

        // Deterministic: same non-existent path produces the same hash.
        let id2 = fallback_project_id_from_path(&nonexistent_cas).unwrap();
        assert_eq!(id, id2);
    }

    #[cfg(unix)]
    #[test]
    fn test_fallback_resolves_symlinks_to_same_id() {
        // Documented contract: "symlinked and renamed paths produce the same ID"
        // via `std::fs::canonicalize`. Create a real project, symlink to it,
        // and assert both paths produce the same fallback hash.
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let real_project = temp.path().join("real-project");
        let real_cas = real_project.join(".cas");
        std::fs::create_dir_all(&real_cas).unwrap();

        let link_project = temp.path().join("link-to-project");
        symlink(&real_project, &link_project).unwrap();
        let link_cas = link_project.join(".cas");

        let id_real = fallback_project_id_from_path(&real_cas).unwrap();
        let id_link = fallback_project_id_from_path(&link_cas).unwrap();
        assert_eq!(
            id_real, id_link,
            "symlinked and real paths should hash to the same ID after canonicalization"
        );
    }

    /// Regression test for cas-2c77: OnceLock cached None permanently, so a
    /// transient `find_cas_root()` failure during daemon startup locked out
    /// project scoping for the entire process lifetime.
    ///
    /// This test reproduces the exact contract using the same Mutex<Option>
    /// pattern as the production code. We can't safely test the process-global
    /// static (env var mutations race with parallel tests), so we verify the
    /// pattern in isolation: None results are retried, Some results are cached.
    #[test]
    fn test_mutex_cache_retries_none_but_caches_some() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicU32, Ordering};

        let cache: Mutex<Option<String>> = Mutex::new(None);
        let call_count = AtomicU32::new(0);

        // Simulate the get_project_canonical_id pattern with a controllable resolver
        let get_id = |resolver: &dyn Fn() -> Option<String>| -> Option<String> {
            let mut cached = cache.lock().unwrap();
            if let Some(ref id) = *cached {
                return Some(id.clone());
            }
            call_count.fetch_add(1, Ordering::SeqCst);
            let result = resolver();
            if result.is_some() {
                *cached = result.clone();
            }
            result
        };

        // First call: resolver returns None (simulates find_cas_root failing)
        let result1 = get_id(&|| None);
        assert_eq!(result1, None);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second call: resolver still returns None — should retry (not return cached None)
        let result2 = get_id(&|| None);
        assert_eq!(result2, None);
        assert_eq!(call_count.load(Ordering::SeqCst), 2, "None must not be cached — resolver should be called again");

        // Third call: resolver now succeeds (simulates cwd moved into a CAS project)
        let result3 = get_id(&|| Some("my-project".to_string()));
        assert_eq!(result3, Some("my-project".to_string()));
        assert_eq!(call_count.load(Ordering::SeqCst), 3);

        // Fourth call: should return cached value without calling resolver
        let result4 = get_id(&|| panic!("resolver should not be called when cache has Some"));
        assert_eq!(result4, Some("my-project".to_string()));
        assert_eq!(call_count.load(Ordering::SeqCst), 3, "Some must be cached — resolver should not be called again");
    }

    #[test]
    fn test_team_sync_timestamps_persist() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cloud.json");

        let mut config = CloudConfig {
            token: Some("test_token".to_string()),
            ..Default::default()
        };
        config.set_team("team-123", "my-team");
        let ts = Utc::now();
        config.set_team_sync_timestamp("team-123", ts);

        config.save_to(&path).unwrap();

        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(loaded.team_id, Some("team-123".to_string()));
        assert_eq!(loaded.team_slug, Some("my-team".to_string()));
        // Timestamps are stored with second precision in JSON
        let loaded_ts = loaded.get_team_sync_timestamp("team-123").unwrap();
        assert!((loaded_ts - ts).num_seconds().abs() < 1);
    }

    // cas-1ced: git-remote URL normalizer + config.toml round-trip helpers.

    #[test]
    fn normalize_https_strips_protocol_and_dot_git() {
        assert_eq!(
            normalize_git_remote_url("https://github.com/foo/bar.git").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_https_handles_missing_dot_git() {
        assert_eq!(
            normalize_git_remote_url("https://github.com/foo/bar").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_http_strips_protocol_and_dot_git() {
        assert_eq!(
            normalize_git_remote_url("http://gitlab.example.com/g/p.git").as_deref(),
            Some("gitlab.example.com/g/p"),
        );
    }

    #[test]
    fn normalize_ssh_user_form() {
        assert_eq!(
            normalize_git_remote_url("git@github.com:foo/bar.git").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_ssh_url_form() {
        assert_eq!(
            normalize_git_remote_url("ssh://git@github.com/foo/bar.git").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_gitlab_subgroup() {
        assert_eq!(
            normalize_git_remote_url("https://gitlab.com/group/subgroup/project.git").as_deref(),
            Some("gitlab.com/group/subgroup/project"),
        );
    }

    #[test]
    fn normalize_rejects_local_path() {
        // Local path is not a recognizable URL shape — falls through to None.
        assert_eq!(normalize_git_remote_url("/home/user/repo"), None);
    }

    #[test]
    fn normalize_rejects_empty() {
        assert_eq!(normalize_git_remote_url(""), None);
        assert_eq!(normalize_git_remote_url("   "), None);
    }

    #[test]
    fn config_toml_roundtrip_writes_and_reads_canonical_id() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path();
        assert_eq!(canonical_id_from_config_toml(cas_root), None);
        set_canonical_id_in_config_toml(cas_root, "github.com/foo/bar").unwrap();
        assert_eq!(
            canonical_id_from_config_toml(cas_root).as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn config_toml_preserves_other_sections() {
        // Seed config.toml with a pre-existing block that has nothing to do
        // with [project]. The write must NOT clobber it.
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path();
        std::fs::write(
            cas_root.join("config.toml"),
            "[memory]\nsession_learn_auto = true\n",
        )
        .unwrap();

        set_canonical_id_in_config_toml(cas_root, "github.com/foo/bar").unwrap();

        let content = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
        assert!(content.contains("session_learn_auto"), "pre-existing [memory] block must survive — got:\n{content}");
        assert!(content.contains("github.com/foo/bar"), "new canonical_id must be written — got:\n{content}");
    }

    #[test]
    fn resolve_canonical_id_prefers_config_toml_over_folder_name() {
        // Lock in the resolution-order change: config.toml beats folder name.
        let temp = tempfile::tempdir().unwrap();
        // Create the `.cas/` subdir so cas_root looks like a real CAS root
        // (parent dir name = `quiet-leopard-46` or whatever — irrelevant).
        let cas_root = temp.path().join("project-dir");
        std::fs::create_dir_all(&cas_root).unwrap();
        set_canonical_id_in_config_toml(&cas_root, "github.com/owner/explicit").unwrap();

        assert_eq!(
            resolve_canonical_id(&cas_root).as_deref(),
            Some("github.com/owner/explicit"),
            "config.toml [project] canonical_id must win over folder-name fallback",
        );
    }
}
