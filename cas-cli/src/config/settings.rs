use cas_factory::AutoPromptConfig;
use serde::{Deserialize, Serialize};

/// Sync configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Whether auto-sync is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Target directory for synced rules (relative to project root)
    #[serde(default = "default_target")]
    pub target: String,

    /// Minimum helpful votes before syncing
    #[serde(default = "default_min_helpful")]
    pub min_helpful: i32,
}

/// Task configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksConfig {
    /// Nudge to commit changes when closing a task
    #[serde(default)]
    pub commit_nudge_on_close: bool,

    /// Block agent exit while open tasks remain (claimed tasks, epic subtasks, session-created)
    #[serde(default = "default_true")]
    pub block_exit_on_open: bool,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            commit_nudge_on_close: false,
            block_exit_on_open: true,
        }
    }
}

/// Factory configuration for multi-agent sessions (native TUI)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationConfig {
    /// Default number of worker agents (default: 0 for supervisor-only startup)
    #[serde(default = "default_orchestration_pane_count")]
    pub default_workers: u8,

    /// Auto-prompting configuration for factory events
    #[serde(default)]
    pub auto_prompt: AutoPromptConfig,
}

fn default_orchestration_pane_count() -> u8 {
    0 // Supervisor-only by default for EPIC planning
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            default_workers: default_orchestration_pane_count(),
            auto_prompt: AutoPromptConfig::default(),
        }
    }
}

/// Factory mode configuration for supervisor task assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryConfig {
    /// Warn when assigning tasks to workers with stale worktrees
    #[serde(default = "default_true")]
    pub warn_stale_assignment: bool,

    /// Block task assignment to workers with stale worktrees (if commits behind >= threshold)
    #[serde(default)]
    pub block_stale_assignment: bool,

    /// Number of commits behind the sync target before considering a worktree stale
    #[serde(default = "default_stale_threshold")]
    pub stale_threshold_commits: u32,

    /// Cap on `CARGO_BUILD_JOBS` exported into each factory worker's env.
    ///
    /// Purpose: prevent the multi-worker cargo thundering-herd that
    /// saturates the host and wedges Claude Code workers in the JS
    /// crash-screen state (cas-4513 + cas-0bf4). Each worker runs its
    /// own `target/` dir and its own rustc jobs; without this cap,
    /// peak concurrency is `workers × num_cpus` rustc threads.
    ///
    /// - `"auto"` (default): cas-pty computes
    ///   `max(2, available_parallelism() / 4)`. The "÷4" assumes up to
    ///   4 concurrent workers — the common factory-mode topology on a
    ///   16-thread dev box. Override via `CAS_FACTORY_CARGO_BUILD_JOBS`
    ///   if the supervisor's scale differs.
    /// - Any numeric string (e.g. `"4"`): exported verbatim.
    #[serde(default = "default_auto")]
    pub cargo_build_jobs: String,

    /// When true, prefix each worker's spawn command with `nice -n 10`
    /// so cargo-driven rustc jobs run at a lower scheduling priority
    /// than the supervisor's Claude Code event loop. Workers still
    /// contend equally among themselves, but the supervisor pane
    /// stays responsive under load — which is what keeps the factory
    /// steerable when a worker storm starts.
    ///
    /// Default `true`. Flip to `false` for single-worker sessions or
    /// when benchmarking, since the priority drop does slow individual
    /// cargo builds under contention.
    #[serde(default = "default_true")]
    pub nice_cargo: bool,
}

fn default_stale_threshold() -> u32 {
    1
}

fn default_auto() -> String {
    "auto".to_string()
}

impl Default for FactoryConfig {
    fn default() -> Self {
        Self {
            warn_stale_assignment: true,
            block_stale_assignment: true,
            stale_threshold_commits: default_stale_threshold(),
            cargo_build_jobs: default_auto(),
            nice_cargo: true,
        }
    }
}

/// Code indexing configuration for background code indexing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeConfig {
    /// Whether background code indexing is enabled
    #[serde(default)]
    pub enabled: bool,

    /// Paths to watch for code changes (relative to project root)
    #[serde(default = "default_code_watch_paths")]
    pub watch_paths: Vec<String>,

    /// Glob patterns for directories/files to exclude from indexing
    #[serde(default = "default_code_exclude_patterns")]
    pub exclude_patterns: Vec<String>,

    /// File extensions to index (without leading dot)
    #[serde(default = "default_code_extensions")]
    pub extensions: Vec<String>,

    /// How often to run full code indexing (seconds)
    #[serde(default = "default_code_index_interval")]
    pub index_interval_secs: u64,

    /// Debounce time for file watcher events (milliseconds)
    #[serde(default = "default_code_debounce")]
    pub debounce_ms: u64,
}

fn default_code_watch_paths() -> Vec<String> {
    vec!["src".into(), "lib".into(), "crates".into()]
}

fn default_code_exclude_patterns() -> Vec<String> {
    vec![
        "target/**".into(),
        "node_modules/**".into(),
        ".git/**".into(),
        "dist/**".into(),
        "build/**".into(),
        "_build/**".into(),
        "deps/**".into(),
        "vendor/**".into(),
    ]
}

fn default_code_extensions() -> Vec<String> {
    vec![
        "rs".into(),
        "ts".into(),
        "tsx".into(),
        "js".into(),
        "jsx".into(),
        "py".into(),
        "go".into(),
        "ex".into(),
        "exs".into(),
        "rb".into(),
        "java".into(),
        "kt".into(),
        "swift".into(),
    ]
}

fn default_code_index_interval() -> u64 {
    60 // 1 minute
}

fn default_code_debounce() -> u64 {
    500 // 500ms
}

impl Default for CodeConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Opt-in for CPU-intensive feature
            watch_paths: default_code_watch_paths(),
            exclude_patterns: default_code_exclude_patterns(),
            extensions: default_code_extensions(),
            index_interval_secs: default_code_index_interval(),
            debounce_ms: default_code_debounce(),
        }
    }
}

/// Notification configuration for TUI alerts and hook notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Master switch for notifications
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Play terminal bell on new notifications
    #[serde(default = "default_true")]
    pub sound_enabled: bool,

    /// How long to display notifications (seconds)
    #[serde(default = "default_display_duration")]
    pub display_duration_secs: u64,

    /// Maximum notifications to display at once
    #[serde(default = "default_max_visible")]
    pub max_visible: usize,

    /// Task notification settings
    #[serde(default)]
    pub tasks: TaskNotifications,

    /// Entry/memory notification settings
    #[serde(default)]
    pub entries: EntryNotifications,

    /// Rule notification settings
    #[serde(default)]
    pub rules: RuleNotifications,

    /// Skill notification settings
    #[serde(default)]
    pub skills: SkillNotifications,

    // === Hook notification settings (for Notification hook) ===
    /// Notify on permission prompts (Claude needs user approval)
    #[serde(default)]
    pub on_permission_prompt: bool,

    /// Notify when Claude is idle and waiting for input
    #[serde(default)]
    pub on_idle_prompt: bool,

    /// Notify on successful authentication
    #[serde(default)]
    pub on_auth_success: bool,

    /// Optional webhook URL for Slack/Discord integration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
}

/// Task-specific notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotifications {
    /// Notify when a task is created
    #[serde(default = "default_true")]
    pub on_created: bool,

    /// Notify when a task is started
    #[serde(default = "default_true")]
    pub on_started: bool,

    /// Notify when a task is closed
    #[serde(default = "default_true")]
    pub on_closed: bool,

    /// Notify when a task is updated (off by default - too noisy)
    #[serde(default)]
    pub on_updated: bool,
}

impl Default for TaskNotifications {
    fn default() -> Self {
        Self {
            on_created: true,
            on_started: true,
            on_closed: true,
            on_updated: false,
        }
    }
}

/// Entry/memory notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryNotifications {
    /// Notify when an entry is added
    #[serde(default = "default_true")]
    pub on_added: bool,

    /// Notify when an entry is updated (off by default)
    #[serde(default)]
    pub on_updated: bool,

    /// Notify when an entry is deleted
    #[serde(default = "default_true")]
    pub on_deleted: bool,
}

impl Default for EntryNotifications {
    fn default() -> Self {
        Self {
            on_added: true,
            on_updated: false,
            on_deleted: true,
        }
    }
}

/// Rule notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleNotifications {
    /// Notify when a rule is created
    #[serde(default = "default_true")]
    pub on_created: bool,

    /// Notify when a rule is promoted to Proven
    #[serde(default = "default_true")]
    pub on_promoted: bool,

    /// Notify when a rule is demoted (off by default)
    #[serde(default)]
    pub on_demoted: bool,
}

impl Default for RuleNotifications {
    fn default() -> Self {
        Self {
            on_created: true,
            on_promoted: true,
            on_demoted: false,
        }
    }
}

/// Skill notification settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillNotifications {
    /// Notify when a skill is created
    #[serde(default = "default_true")]
    pub on_created: bool,

    /// Notify when a skill is enabled
    #[serde(default = "default_true")]
    pub on_enabled: bool,

    /// Notify when a skill is disabled (off by default)
    #[serde(default)]
    pub on_disabled: bool,
}

impl Default for SkillNotifications {
    fn default() -> Self {
        Self {
            on_created: true,
            on_enabled: true,
            on_disabled: false,
        }
    }
}

fn default_display_duration() -> u64 {
    5 // 5 seconds
}

fn default_max_visible() -> usize {
    3
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sound_enabled: true,
            display_duration_secs: default_display_duration(),
            max_visible: default_max_visible(),
            tasks: TaskNotifications::default(),
            entries: EntryNotifications::default(),
            rules: RuleNotifications::default(),
            skills: SkillNotifications::default(),
            // Hook notification settings (disabled by default)
            on_permission_prompt: false,
            on_idle_prompt: false,
            on_auth_success: false,
            webhook_url: None,
        }
    }
}

/// Cloud sync configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudSyncConfig {
    /// Whether auto-sync is enabled (when logged in)
    #[serde(default = "default_true")]
    pub auto_sync: bool,

    /// How often to sync (seconds)
    #[serde(default = "default_cloud_sync_interval")]
    pub interval_secs: u64,

    /// Pull from cloud on MCP server startup
    #[serde(default = "default_true")]
    pub pull_on_start: bool,

    /// Maximum retry attempts for failed syncs
    #[serde(default = "default_max_retries")]
    pub max_retries: i32,
}

fn default_cloud_sync_interval() -> u64 {
    60 // 1 minute
}

fn default_max_retries() -> i32 {
    5
}

impl Default for CloudSyncConfig {
    fn default() -> Self {
        Self {
            auto_sync: true,
            interval_secs: default_cloud_sync_interval(),
            pull_on_start: true,
            max_retries: default_max_retries(),
        }
    }
}

/// Development mode configuration for tracing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevConfig {
    /// Enable dev mode tracing
    #[serde(default)]
    pub dev_mode: bool,

    /// Trace CLI command executions
    #[serde(default = "default_true")]
    pub trace_commands: bool,

    /// Trace store operations (add, update, delete, get)
    #[serde(default = "default_true")]
    pub trace_store_ops: bool,

    /// Trace Claude API calls with full prompts/responses
    #[serde(default = "default_true")]
    pub trace_claude_api: bool,

    /// Trace hook events
    #[serde(default = "default_true")]
    pub trace_hooks: bool,

    /// Days to retain traces before auto-cleanup
    #[serde(default = "default_trace_retention")]
    pub trace_retention_days: i64,
}

fn default_trace_retention() -> i64 {
    7
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            dev_mode: false,
            trace_commands: true,
            trace_store_ops: true,
            trace_claude_api: true,
            trace_hooks: true,
            trace_retention_days: 7,
        }
    }
}

/// Telemetry configuration for anonymous usage tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether telemetry is enabled (default: false, opt-in via CAS_TELEMETRY=1)
    #[serde(default)]
    pub enabled: bool,

    /// Anonymous user ID (generated on first run)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous_id: Option<String>,

    /// Whether user has given consent for telemetry (None = not asked yet)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consent_given: Option<bool>,
}

/// LLM configuration for harness and model selection
///
/// Controls which CLI harness (Claude or Codex) is used and which model
/// each harness runs. Per-role overrides allow different configurations
/// for supervisor vs worker agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Which CLI harness to use: "claude" or "codex"
    #[serde(default = "default_harness")]
    pub harness: String,

    /// Model to use within the harness (e.g., "claude-sonnet-4-5-20250929", "gpt-5.3-codex")
    /// If not set, the harness uses its default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Reasoning effort level: "low", "medium", or "high" (only supported by some models)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,

    /// Override configuration for supervisor agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor: Option<LlmRoleConfig>,

    /// Override configuration for worker agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<LlmRoleConfig>,
}

/// Per-role LLM overrides (supervisor or worker)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmRoleConfig {
    /// Override harness for this role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,

    /// Override model for this role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Override reasoning effort for this role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

fn default_harness() -> String {
    "claude".to_string()
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            harness: default_harness(),
            model: None,
            reasoning_effort: None,
            supervisor: None,
            worker: None,
        }
    }
}

impl LlmConfig {
    /// Resolve the harness for a given role, falling back to the top-level setting.
    pub fn harness_for_role(&self, role: &str) -> &str {
        let role_override = match role {
            "supervisor" => self.supervisor.as_ref().and_then(|r| r.harness.as_deref()),
            "worker" => self.worker.as_ref().and_then(|r| r.harness.as_deref()),
            _ => None,
        };
        role_override.unwrap_or(&self.harness)
    }

    /// Resolve the model for a given role, falling back to the top-level setting.
    pub fn model_for_role(&self, role: &str) -> Option<&str> {
        let role_override = match role {
            "supervisor" => self.supervisor.as_ref().and_then(|r| r.model.as_deref()),
            "worker" => self.worker.as_ref().and_then(|r| r.model.as_deref()),
            _ => None,
        };
        role_override.or(self.model.as_deref())
    }

    /// Resolve the reasoning effort for a given role, falling back to the top-level setting.
    pub fn reasoning_effort_for_role(&self, role: &str) -> Option<&str> {
        let role_override = match role {
            "supervisor" => self
                .supervisor
                .as_ref()
                .and_then(|r| r.reasoning_effort.as_deref()),
            "worker" => self
                .worker
                .as_ref()
                .and_then(|r| r.reasoning_effort.as_deref()),
            _ => None,
        };
        role_override.or(self.reasoning_effort.as_deref())
    }
}

fn default_true() -> bool {
    true
}

fn default_target() -> String {
    ".claude/rules/cas".to_string()
}

fn default_min_helpful() -> i32 {
    1
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            target: ".claude/rules/cas".to_string(),
            min_helpful: 1,
        }
    }
}

/// `[integrations]` — gates Phase-3 doctor-and-banner behavior for the
/// vercel/neon/github auto-integration family (EPIC cas-b65f). Default-off
/// across the board so an absent or empty section preserves the prior UX.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationsConfig {
    /// When true, the SessionStart hook surfaces a low-severity banner if
    /// any platform reports stale IDs. Default `false` — the codemap
    /// freshness banner already occupies the SessionStart slot, and
    /// stacking another banner there erodes its signal. Opt-in only.
    #[serde(default)]
    pub session_start_warn: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-0bf4: defaults for the two resource-contention knobs must stay
    /// stable — they ship as the on-by-default mitigation for factory
    /// worker wedges. A careless refactor that flipped `nice_cargo` to
    /// `false` or `cargo_build_jobs` to `""` would silently disable the
    /// cap on every new install.
    #[test]
    fn factory_config_defaults_cargo_contention_knobs() {
        let fc = FactoryConfig::default();
        assert_eq!(
            fc.cargo_build_jobs, "auto",
            "cargo_build_jobs default must be 'auto' so cas-pty computes the cap"
        );
        assert!(
            fc.nice_cargo,
            "nice_cargo default must be true so workers run niced relative to supervisor"
        );
    }

    /// Round-trip: a persisted config with no factory section deserializes
    /// to the same defaults as `Default::default()`. Guards against a
    /// future serde attribute mismatch that would change read-vs-default
    /// divergence (the classic "silent config drift" bug).
    #[test]
    fn factory_config_roundtrips_through_toml_empty_section() {
        let toml_str = "[factory]\n";
        let parsed: std::collections::HashMap<String, FactoryConfig> =
            toml::from_str(toml_str).expect("valid toml");
        let fc = parsed.get("factory").expect("section present");
        assert_eq!(fc.cargo_build_jobs, FactoryConfig::default().cargo_build_jobs);
        assert_eq!(fc.nice_cargo, FactoryConfig::default().nice_cargo);
    }

    // ── LlmConfig::reasoning_effort_for_role ─────────────────────────────────
    // cas-9393: critical-path method feeds supervisor_effort and worker_effort
    // through the factory spawn pipeline — must have full coverage.

    /// No config at all → None for every role.
    #[test]
    fn reasoning_effort_for_role_no_config_returns_none() {
        let llm = LlmConfig::default();
        assert_eq!(llm.reasoning_effort_for_role("supervisor"), None);
        assert_eq!(llm.reasoning_effort_for_role("worker"), None);
    }

    /// Top-level `reasoning_effort` is the fallback when no per-role override
    /// is present. Both supervisor and worker should see it.
    #[test]
    fn reasoning_effort_for_role_top_level_fallback() {
        let llm = LlmConfig {
            reasoning_effort: Some("medium".to_string()),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("medium"),
            "supervisor should fall back to top-level reasoning_effort"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("medium"),
            "worker should fall back to top-level reasoning_effort"
        );
    }

    /// A supervisor-specific override shadows the top-level value for the
    /// supervisor role, while the worker still sees the top-level value.
    #[test]
    fn reasoning_effort_for_role_supervisor_override() {
        let llm = LlmConfig {
            reasoning_effort: Some("high".to_string()),
            supervisor: Some(LlmRoleConfig {
                reasoning_effort: Some("low".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("low"),
            "supervisor override must shadow top-level value"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker must still see top-level when no worker override is set"
        );
    }

    /// A worker-specific override shadows the top-level value for the worker
    /// role, while the supervisor still sees the top-level value.
    #[test]
    fn reasoning_effort_for_role_worker_override() {
        let llm = LlmConfig {
            reasoning_effort: Some("medium".to_string()),
            worker: Some(LlmRoleConfig {
                reasoning_effort: Some("high".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker override must shadow top-level value"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("medium"),
            "supervisor must still see top-level when no supervisor override is set"
        );
    }

    /// Per-role overrides are independent: supervisor and worker can each have
    /// their own distinct effort level without interfering with each other.
    #[test]
    fn reasoning_effort_for_role_independent_overrides() {
        let llm = LlmConfig {
            reasoning_effort: None,
            supervisor: Some(LlmRoleConfig {
                reasoning_effort: Some("low".to_string()),
                ..LlmRoleConfig::default()
            }),
            worker: Some(LlmRoleConfig {
                reasoning_effort: Some("high".to_string()),
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("low"),
            "supervisor-only override must not bleed into worker"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker-only override must not bleed into supervisor"
        );
    }

    /// An unknown / unrecognised role is treated as having no per-role override.
    /// It falls back to the top-level value, or None if the top-level is unset.
    #[test]
    fn reasoning_effort_for_role_unknown_role_falls_back_to_top_level() {
        let llm_with_top = LlmConfig {
            reasoning_effort: Some("medium".to_string()),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm_with_top.reasoning_effort_for_role("orchestrator"),
            Some("medium"),
            "unknown role must fall back to top-level reasoning_effort"
        );

        let llm_no_top = LlmConfig::default();
        assert_eq!(
            llm_no_top.reasoning_effort_for_role("orchestrator"),
            None,
            "unknown role with no top-level must return None"
        );
    }

    /// A per-role block may exist (e.g. to override harness or model) without
    /// setting `reasoning_effort`. In that case the top-level value must still
    /// be returned — the `and_then` short-circuit must not swallow the fallback.
    #[test]
    fn reasoning_effort_for_role_partial_override_falls_back_to_top_level() {
        let llm = LlmConfig {
            reasoning_effort: Some("high".to_string()),
            supervisor: Some(LlmRoleConfig {
                harness: Some("codex".to_string()),
                reasoning_effort: None, // effort NOT set in the role block
                ..LlmRoleConfig::default()
            }),
            ..LlmConfig::default()
        };
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("high"),
            "partial role override (harness set, effort absent) must fall back to top-level"
        );
    }

    /// Round-trip via TOML deserialization: verifies that the serde attributes
    /// on `LlmRoleConfig::reasoning_effort` are correct and that the field is
    /// not silently dropped during deserialization.
    #[test]
    fn reasoning_effort_for_role_toml_roundtrip() {
        let toml_str = r#"
[llm]
reasoning_effort = "medium"

[llm.supervisor]
reasoning_effort = "low"

[llm.worker]
reasoning_effort = "high"
"#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            llm: LlmConfig,
        }
        let parsed: Wrapper = toml::from_str(toml_str).expect("valid toml");
        let llm = parsed.llm;
        assert_eq!(
            llm.reasoning_effort_for_role("supervisor"),
            Some("low"),
            "supervisor reasoning_effort must survive TOML deserialization"
        );
        assert_eq!(
            llm.reasoning_effort_for_role("worker"),
            Some("high"),
            "worker reasoning_effort must survive TOML deserialization"
        );
        // Top-level fallback still works after round-trip
        assert_eq!(
            llm.reasoning_effort_for_role("orchestrator"),
            Some("medium"),
            "top-level reasoning_effort must survive TOML deserialization"
        );
    }
}
