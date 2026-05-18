//! Native Agent Teams integration for factory daemon.
//!
//! Manages Claude Code's native Agent Teams file structure:
//! - `~/.claude/teams/{team-name}/config.json` — team member registry
//! - `~/.claude/teams/{team-name}/inboxes/{agent-name}.json` — per-agent inbox files
//!
//! This replaces the old prompt_queue + mux.inject (PTY stdin injection) transport
//! with native Teams mailbox writes that Claude Code polls internally.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Colors assigned to team members (matches Claude Code's palette).
const AGENT_COLORS: &[&str] = &["green", "blue", "yellow", "cyan", "magenta", "red", "white"];

/// The director agent name registered in the team config.
/// The daemon uses this identity when writing system/auto-prompt messages
/// to agent inboxes so that Claude Code recognizes the sender as a valid
/// team member.
pub const DIRECTOR_AGENT_NAME: &str = "director";

/// Dedup window for identical (from, text) inbox writes (task cas-7f57).
///
/// The daemon can re-fire the same auto-prompt (e.g. "You have been assigned
/// cas-X") in a number of documented-but-unintended paths — event-detector
/// last_state resets across refresh ticks, prompt_queue retry loops on pane
/// busy, teams-inbox outbox replays on client reconnect. Workers live-reported
/// receiving dozens of identical assignment messages minutes apart for tasks
/// that were already Closed, each costing ~100–500 tokens of context per
/// repeat. This is the coordination-layer guard: if the target's inbox
/// already contains a message with identical `from` + `text` within the last
/// `INBOX_DEDUP_WINDOW`, we skip the append and no-op the write. The deeper
/// causes are left as follow-ups; this stops the bleeding at the delivery
/// boundary.
const INBOX_DEDUP_WINDOW: chrono::Duration = chrono::Duration::minutes(10);

/// Retention window for messages in the inbox file (task cas-7f57).
///
/// Inbox files are append-only today and `read: false` is never flipped to
/// true (see history comment on the field). Without pruning, every session
/// boot replays the entire accumulated history to Claude Code. On every
/// write we drop messages older than this window so the file stays bounded
/// and stale messages cannot haunt future sessions.
const INBOX_RETENTION: chrono::Duration = chrono::Duration::hours(2);

/// A single message in a Teams inbox file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxMessage {
    pub from: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub timestamp: String,
    pub color: String,
    pub read: bool,
}

/// Team member entry in config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub agent_id: String,
    pub name: String,
    pub agent_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_mode_required: Option<bool>,
    pub joined_at: u64,
    pub tmux_pane_id: String,
    pub cwd: String,
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_type: Option<String>,
}

/// Team config.json structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConfig {
    pub name: String,
    pub description: String,
    pub created_at: u64,
    pub lead_agent_id: String,
    pub lead_session_id: String,
    pub members: Vec<TeamMember>,
}

/// Manages the native Agent Teams file structure for a factory session.
pub struct TeamsManager {
    team_name: String,
    teams_dir: PathBuf,
    inboxes_dir: PathBuf,
}

impl TeamsManager {
    /// Create a new TeamsManager for the given factory session.
    ///
    /// The team name is derived from the session name.
    /// Files are stored at `~/.claude/teams/{team-name}/`.
    pub fn new(session_name: &str) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let teams_dir = home.join(".claude").join("teams").join(session_name);
        let inboxes_dir = teams_dir.join("inboxes");

        Self {
            team_name: session_name.to_string(),
            teams_dir,
            inboxes_dir,
        }
    }

    /// Get the team name.
    pub fn team_name(&self) -> &str {
        &self.team_name
    }

    /// Format an agent ID: `{name}@{team-name}`.
    pub fn agent_id_for(&self, name: &str) -> String {
        format!("{}@{}", name, self.team_name)
    }

    /// Build a teams_configs HashMap for MuxConfig before agents are spawned.
    ///
    /// This is a static method because it's called before TeamsManager is fully
    /// initialized (before `init_team_config`). It constructs the CLI flags map
    /// that Mux::factory() uses when spawning agent PTYs.
    /// Returns `(configs_map, lead_session_id)`.
    pub fn build_configs_for_mux(
        session_name: &str,
        supervisor_name: &str,
        worker_names: &[String],
    ) -> (
        std::collections::HashMap<String, cas_mux::TeamsSpawnConfig>,
        String,
    ) {
        let mut configs = std::collections::HashMap::new();
        let lead_session_id = uuid::Uuid::new_v4().to_string();

        // Supervisor settings path — auto-allow the filesystem tool families
        // so the supervisor's tool calls don't get forwarded to itself via
        // team permission routing (self-leadership deadlock). Workers get
        // the same treatment below via per-worker settings files (cas-e15d)
        // to avoid the symmetric phantom `team-lead` hang.
        //
        // The file is written *here*, eagerly, because the factory spawns the
        // supervisor PTY during `FactoryApp::new` — which runs *before*
        // `init_team_config` in the daemon boot sequence. If we deferred the
        // write, `claude` would be launched with `--settings <path>` pointing
        // at a file that doesn't yet exist and would silently skip our
        // allowlist, recreating the deadlock. Writing here keeps the
        // invariant "path in TeamsSpawnConfig implies file exists on disk".
        let supervisor_settings_path_buf = Self::supervisor_settings_path_for(session_name);
        let supervisor_settings_path = supervisor_settings_path_buf.to_string_lossy().to_string();
        if let Err(e) = Self::write_supervisor_settings_to(&supervisor_settings_path_buf) {
            // Downgrade to a warning so the factory still boots on transient
            // I/O issues; if the write fails the supervisor falls back to
            // the pre-fix (deadlock-prone) behavior but everything else
            // proceeds and the log makes post-hoc diagnosis obvious.
            tracing::warn!(
                "Failed to pre-write supervisor settings at {:?}: {}",
                supervisor_settings_path_buf,
                e
            );
        }

        // Supervisor — keyed by pane name for PTY lookup, but agent_name is
        // always "supervisor" so Claude identifies as "supervisor" in the team.
        configs.insert(
            supervisor_name.to_string(),
            cas_mux::TeamsSpawnConfig {
                team_name: session_name.to_string(),
                agent_id: format!("supervisor@{}", session_name),
                agent_name: "supervisor".to_string(),
                agent_color: "green".to_string(),
                agent_type: "team-lead".to_string(),
                parent_session_id: None,
                lead_session_id: Some(lead_session_id.clone()),
                settings_path: Some(supervisor_settings_path),
            },
        );

        // Workers — each gets its own per-worker settings file so
        // filesystem tool calls auto-approve instead of escalating to the
        // phantom `team-lead` mailbox. Same eager-write invariant as the
        // supervisor: the file must exist on disk *before* the PTY spawns
        // with `--settings <path>` or claude silently falls back to the
        // stock allowlist.
        for (i, name) in worker_names.iter().enumerate() {
            let worker_settings_path_buf = Self::worker_settings_path_for(session_name, name);
            let worker_settings_path =
                worker_settings_path_buf.to_string_lossy().to_string();
            if let Err(e) = Self::write_worker_settings_to(&worker_settings_path_buf) {
                tracing::warn!(
                    "Failed to pre-write worker settings for {} at {:?}: {}",
                    name,
                    worker_settings_path_buf,
                    e
                );
            }

            configs.insert(
                name.clone(),
                cas_mux::TeamsSpawnConfig {
                    team_name: session_name.to_string(),
                    agent_id: format!("{}@{}", name, session_name),
                    agent_name: name.clone(),
                    agent_color: Self::color_for_index(i).to_string(),
                    agent_type: "general-purpose".to_string(),
                    parent_session_id: Some(lead_session_id.clone()),
                    lead_session_id: None,
                    settings_path: Some(worker_settings_path),
                },
            );
        }

        (configs, lead_session_id)
    }

    /// Compute the on-disk path of the supervisor-only settings file for a
    /// given session name. The file lives alongside `config.json` under
    /// `~/.claude/teams/{session}/supervisor-settings.json` and is written by
    /// [`Self::build_configs_for_mux`] (eagerly, before PTY spawn) and
    /// re-written by [`Self::init_team_config`] (idempotent rewrite after the
    /// team directory is fully populated). See [`supervisor_settings_contents`]
    /// for the allowlist shape.
    pub fn supervisor_settings_path_for(session_name: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".claude")
            .join("teams")
            .join(session_name)
            .join("supervisor-settings.json")
    }

    /// Write `supervisor-settings.json` at the given absolute path, creating
    /// the parent directory if needed. Static variant used by
    /// [`Self::build_configs_for_mux`], which runs before any instance of
    /// `TeamsManager` is constructed. Idempotent and safe to call repeatedly.
    pub fn write_supervisor_settings_to(path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(&Self::supervisor_settings_contents())?;
        std::fs::write(path, body)?;
        tracing::info!("Wrote supervisor settings at {:?}", path);
        Ok(())
    }

    /// The JSON body of the supervisor settings file — a Claude Code
    /// `permissions.allow` list that auto-approves the four tool families
    /// whose approvals would otherwise route back to the supervisor itself,
    /// plus a `hooks` block wiring `PreToolUse` and `PermissionRequest` to
    /// `cas hook` so the factory auto-approve handlers actually run.
    ///
    /// Kept tight on purpose: no MCP tools, no network, no shell glob expansion
    /// beyond the base tool name. The deadlock only fires for tools that are
    /// not otherwise auto-allowed, and the factory supervisor already runs
    /// with `--dangerously-skip-permissions`, so this list is the minimum set
    /// observed to produce the routing-deadlock symptom.
    ///
    /// The `hooks` block is the load-bearing belt under Claude Code 2.1.x:
    /// the `permissions.allow` list alone does NOT short-circuit the
    /// team-mode UG9 escalation (see `pre_tool.rs` for the disassembly), so
    /// without these hook entries the supervisor self-deadlocks on every
    /// permission gate that is not otherwise auto-approved.
    pub fn supervisor_settings_contents() -> serde_json::Value {
        let mut body = serde_json::json!({
            "permissions": { "allow": Self::factory_allow_list() },
        });
        body.as_object_mut()
            .expect("object literal")
            .insert("hooks".to_string(), Self::factory_hooks_block());
        body
    }

    /// Compute the on-disk path of a worker's settings file. Lives alongside
    /// `config.json` under `~/.claude/teams/{session}/{worker_name}-settings.json`.
    /// Mirrors [`Self::supervisor_settings_path_for`] — same eager-write
    /// invariant applies.
    pub fn worker_settings_path_for(session_name: &str, worker_name: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".claude")
            .join("teams")
            .join(session_name)
            .join(format!("{worker_name}-settings.json"))
    }

    /// Write a worker settings file at the given absolute path, creating the
    /// parent directory if needed. Idempotent.
    pub fn write_worker_settings_to(path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(&Self::worker_settings_contents())?;
        std::fs::write(path, body)?;
        tracing::info!("Wrote worker settings at {:?}", path);
        Ok(())
    }

    /// JSON body of a worker settings file. Covers the filesystem tool
    /// families whose approvals would otherwise escalate to the phantom
    /// `team-lead` mailbox (agentType misread as name, upstream) and hang.
    ///
    /// Kept to the same shape as [`Self::supervisor_settings_contents`] so
    /// both roles share one surface area review. Any tool added here should
    /// also be added to the supervisor list unless we have a specific reason
    /// to diverge.
    pub fn worker_settings_contents() -> serde_json::Value {
        let mut body = serde_json::json!({
            "permissions": { "allow": Self::factory_allow_list() },
        });
        body.as_object_mut()
            .expect("object literal")
            .insert("hooks".to_string(), Self::factory_hooks_block());
        body
    }

    /// Filesystem tool families whose approval would otherwise route to the
    /// phantom `team-lead` mailbox under Claude Code 2.1.x (see UG9 bug in
    /// `pre_tool.rs`). Used by both the `permissions.allow` list and the
    /// `PreToolUse` hook matcher in the per-role settings file.
    ///
    /// MUST stay in sync with `FACTORY_AUTO_APPROVE_TOOLS` in
    /// `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs` — the hook
    /// handler reads that list to decide whether to auto-approve. If they
    /// diverge, ops in this list (but not the hook list) will hang anyway,
    /// and ops in the hook list (but not this matcher) will fire the hook
    /// for nothing.
    fn factory_allow_list() -> &'static [&'static str] {
        &["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"]
    }

    /// `hooks` block for per-role settings files. Wires `PreToolUse` (belt
    /// #2) and `PermissionRequest` (belt #3) to `cas hook <event>`, which is
    /// what actually short-circuits the team-mode leader-escalation deadlock
    /// on Claude Code 2.1.x — the `permissions.allow` list alone does not.
    ///
    /// Emits exec-form `"args"` (cas-9a60).  CC ≥ 2.1.142 required
    /// (anthropics/claude-code#58441 fixed in 2.1.142, verified on 2.1.143).
    ///
    /// Defaults mirror `cli/hook/config_gen.rs`: 2000ms timeout for both.
    /// `PreToolUse` matcher is the filesystem tool list so we don't fire
    /// the hook on unrelated tools (MCP, Agent, etc. still flow through
    /// Claude Code's normal paths).
    fn factory_hooks_block() -> serde_json::Value {
        let matcher = Self::factory_allow_list().join("|");
        serde_json::json!({
            "PreToolUse": [
                {
                    "matcher": matcher,
                    "hooks": [
                        {
                            "type": "command",
                            "args": ["cas", "hook", "PreToolUse"],
                            "timeout": 2000
                        }
                    ]
                }
            ],
            "PermissionRequest": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "args": ["cas", "hook", "PermissionRequest"],
                            "timeout": 2000
                        }
                    ]
                }
            ]
        })
    }

    /// Assign a color to an agent based on its index in the team.
    pub fn color_for_index(index: usize) -> &'static str {
        AGENT_COLORS[index % AGENT_COLORS.len()]
    }

    /// Initialize the team directory and write config.json with supervisor + initial workers.
    ///
    /// `worker_cwds` maps worker names to their actual working directories (worktree paths
    /// when worktrees are enabled). Workers not in the map use `project_cwd` as fallback.
    pub fn init_team_config(
        &self,
        worker_names: &[String],
        project_cwd: &std::path::Path,
        worker_cwds: &std::collections::HashMap<String, std::path::PathBuf>,
        lead_session_id: &str,
    ) -> anyhow::Result<()> {
        // Create directories
        std::fs::create_dir_all(&self.inboxes_dir)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let project_cwd_str = project_cwd.to_string_lossy().to_string();

        let model = Some("claude-opus-4-7".to_string());

        // Supervisor is the team lead but also a teammate so it polls its inbox.
        // Always registered as "supervisor" regardless of the generated pane name.
        let mut members = vec![TeamMember {
            agent_id: self.agent_id_for("supervisor"),
            name: "supervisor".to_string(),
            agent_type: "team-lead".to_string(),
            model: model.clone(),
            prompt: None,
            color: Some("green".to_string()),
            plan_mode_required: None,
            joined_at: now,
            tmux_pane_id: "tmux".to_string(),
            cwd: project_cwd_str.clone(),
            subscriptions: Vec::new(),
            backend_type: Some("tmux".to_string()),
        }];

        // Director is the daemon's identity for system/auto-prompt messages.
        // Registered as a team member so Claude Code accepts messages from it.
        members.push(TeamMember {
            agent_id: self.agent_id_for(DIRECTOR_AGENT_NAME),
            name: DIRECTOR_AGENT_NAME.to_string(),
            agent_type: "director".to_string(),
            model: model.clone(),
            prompt: None,
            color: Some("white".to_string()),
            plan_mode_required: None,
            joined_at: now,
            tmux_pane_id: "tmux".to_string(),
            cwd: project_cwd_str.clone(),
            subscriptions: Vec::new(),
            backend_type: Some("tmux".to_string()),
        });

        // Add workers (each may have its own worktree path)
        for (i, worker_name) in worker_names.iter().enumerate() {
            let worker_cwd = worker_cwds
                .get(worker_name)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| project_cwd_str.clone());

            members.push(TeamMember {
                agent_id: self.agent_id_for(worker_name),
                name: worker_name.clone(),
                agent_type: "general-purpose".to_string(),
                model: model.clone(),
                prompt: None,
                color: Some(Self::color_for_index(i).to_string()),
                plan_mode_required: Some(false),
                joined_at: now,
                tmux_pane_id: "tmux".to_string(),
                cwd: worker_cwd,
                subscriptions: Vec::new(),
                backend_type: Some("tmux".to_string()),
            });
        }

        let config = TeamConfig {
            name: self.team_name.clone(),
            description: format!("CAS factory session {}", self.team_name),
            created_at: now,
            lead_agent_id: self.agent_id_for("supervisor"),
            lead_session_id: lead_session_id.to_string(),
            members,
        };

        let config_path = self.teams_dir.join("config.json");
        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        // Re-write the supervisor-only settings file. `build_configs_for_mux`
        // already wrote it eagerly (before `FactoryApp::new` spawned the
        // supervisor PTY, which is when `--settings <path>` needs to resolve).
        // We rewrite it here defensively so that code paths reaching
        // `init_team_config` without going through `build_configs_for_mux`
        // still end up with a valid file on disk. The write is idempotent.
        self.write_supervisor_settings()?;

        // Create empty inbox files for all agents
        self.ensure_inbox("supervisor")?;
        self.ensure_inbox(DIRECTOR_AGENT_NAME)?;
        for worker_name in worker_names {
            self.ensure_inbox(worker_name)?;
        }

        tracing::info!(
            "Initialized Teams config at {:?} with {} members",
            config_path,
            1 + worker_names.len()
        );

        Ok(())
    }

    /// Write `supervisor-settings.json` in the team directory. Safe to call
    /// multiple times; the content is fixed so repeated writes are idempotent.
    /// Delegates to [`Self::write_supervisor_settings_to`] so the eager-write
    /// path and the `init_team_config` rewrite share a single implementation.
    pub fn write_supervisor_settings(&self) -> anyhow::Result<()> {
        Self::write_supervisor_settings_to(&self.teams_dir.join("supervisor-settings.json"))
    }

    /// Add a new member to the team (e.g., when a worker is spawned dynamically).
    pub fn add_member(
        &self,
        name: &str,
        cwd: &std::path::Path,
        color_index: usize,
    ) -> anyhow::Result<()> {
        let config_path = self.teams_dir.join("config.json");
        let json = std::fs::read_to_string(&config_path)?;
        let mut config: TeamConfig = serde_json::from_str(&json)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        config.members.push(TeamMember {
            agent_id: self.agent_id_for(name),
            name: name.to_string(),
            agent_type: "general-purpose".to_string(),
            model: Some("claude-opus-4-7".to_string()),
            prompt: None,
            color: Some(Self::color_for_index(color_index).to_string()),
            plan_mode_required: Some(false),
            joined_at: now,
            tmux_pane_id: "tmux".to_string(),
            cwd: cwd.to_string_lossy().to_string(),
            subscriptions: Vec::new(),
            backend_type: Some("tmux".to_string()),
        });

        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        self.ensure_inbox(name)?;

        tracing::info!("Added team member '{}' to {}", name, self.team_name);
        Ok(())
    }

    /// Remove a member from the team (e.g., when a worker is shut down).
    pub fn remove_member(&self, name: &str) -> anyhow::Result<()> {
        let config_path = self.teams_dir.join("config.json");
        let json = std::fs::read_to_string(&config_path)?;
        let mut config: TeamConfig = serde_json::from_str(&json)?;

        config.members.retain(|m| m.name != name);

        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        // Remove inbox file
        let inbox_path = self.inboxes_dir.join(format!("{}.json", name));
        let _ = std::fs::remove_file(&inbox_path);

        tracing::info!("Removed team member '{}' from {}", name, self.team_name);
        Ok(())
    }

    /// Write a message to a target agent's inbox file.
    ///
    /// Uses file locking to prevent corruption when multiple writers
    /// (daemon + agents) access the same inbox concurrently.
    pub fn write_to_inbox(
        &self,
        target: &str,
        from: &str,
        message: &str,
        summary: Option<&str>,
        color: Option<&str>,
    ) -> anyhow::Result<()> {
        let inbox_path = self.inboxes_dir.join(format!("{}.json", target));

        // Ensure inbox file exists
        if !inbox_path.exists() {
            std::fs::write(&inbox_path, "[]")?;
        }

        // Use file locking for safe concurrent access
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&inbox_path)?;

        // Acquire exclusive lock
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret != 0 {
            anyhow::bail!(
                "Failed to lock inbox file {:?}: {}",
                inbox_path,
                std::io::Error::last_os_error()
            );
        }

        // Read existing messages
        let mut messages: Vec<InboxMessage> = {
            let content = std::fs::read_to_string(&inbox_path).unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str(&content).unwrap_or_default()
        };

        let now_utc = chrono::Utc::now();
        let now = now_utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let resolved_color = color.unwrap_or("green").to_string();

        // Always set summary — native Claude Code expects it.
        // Fall back to the message text when no explicit summary is provided.
        let resolved_summary = summary.unwrap_or(message).to_string();

        // Prune messages older than INBOX_RETENTION so the file cannot grow
        // unbounded across sessions — see cas-7f57 and the comment on
        // INBOX_RETENTION above.
        //
        // Unread messages (`read: false`) are preserved regardless of age
        // so a supervisor's recovery/unblock prompt to a wedged worker
        // cannot silently evaporate after the 2h window. See
        // memory `feedback_supervisor_stop_message_latency` — stale
        // STOP messages are already a known delivery hazard, and age-only
        // retention would strictly worsen that failure mode.
        let retention_cutoff = now_utc - INBOX_RETENTION;
        let messages_before_retain = messages.len();
        messages.retain(|m| {
            if !m.read {
                return true;
            }
            match chrono::DateTime::parse_from_rfc3339(&m.timestamp) {
                Ok(ts) => ts.with_timezone(&chrono::Utc) >= retention_cutoff,
                // Unparseable timestamp: keep the message rather than
                // silently drop real data; a future migration can clean
                // these up.
                Err(_) => true,
            }
        });
        let retention_pruned = messages.len() != messages_before_retain;

        // Dedup guard: if an identical (from, text) message exists within
        // the dedup window, skip the append. This is the coordination
        // message-dedupe layer called out in the cas-7f57 acceptance
        // criteria — prevents the director/prompt_queue/outbox replay
        // paths from writing the same "You have been assigned cas-X"
        // message to the same worker repeatedly.
        let dedup_cutoff = now_utc - INBOX_DEDUP_WINDOW;
        let is_recent_duplicate = messages.iter().rev().any(|m| {
            if m.from != from || m.text != message {
                return false;
            }
            match chrono::DateTime::parse_from_rfc3339(&m.timestamp) {
                Ok(ts) => ts.with_timezone(&chrono::Utc) >= dedup_cutoff,
                Err(_) => false,
            }
        });

        if is_recent_duplicate {
            tracing::debug!(
                target: "cas::coordination",
                stage = "dedup_skip",
                channel = "teams_inbox",
                from = from,
                target_agent = target,
                "inbox write skipped — identical message within dedup window"
            );
            // Only re-serialize+write if the retention sweep actually
            // removed anything; otherwise this is a pure no-op and we
            // avoid a write storm on hot duplicate senders.
            if retention_pruned {
                let json = serde_json::to_string_pretty(&messages)?;
                std::fs::write(&inbox_path, json)?;
            }
            unsafe { libc::flock(fd, libc::LOCK_UN) };
            return Ok(());
        }

        messages.push(InboxMessage {
            from: from.to_string(),
            text: message.to_string(),
            summary: Some(resolved_summary),
            timestamp: now,
            color: resolved_color,
            read: false,
        });

        // Write back
        let json = serde_json::to_string_pretty(&messages)?;
        std::fs::write(&inbox_path, json)?;

        // Release lock (automatic on drop, but be explicit)
        unsafe { libc::flock(fd, libc::LOCK_UN) };

        tracing::debug!("Wrote message to inbox: {} -> {}", from, target);

        Ok(())
    }

    /// Ensure an inbox file exists for the given agent.
    fn ensure_inbox(&self, name: &str) -> anyhow::Result<()> {
        let inbox_path = self.inboxes_dir.join(format!("{}.json", name));
        if !inbox_path.exists() {
            std::fs::write(&inbox_path, "[]")?;
        }
        Ok(())
    }

    /// Clean up the team directory on shutdown.
    pub fn cleanup(&self) {
        if self.teams_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.teams_dir) {
                tracing::warn!("Failed to clean up teams dir {:?}: {}", self.teams_dir, e);
            } else {
                tracing::info!("Cleaned up teams dir {:?}", self.teams_dir);
            }
        }
    }

    /// Remove orphaned team directories whose daemon is no longer running.
    ///
    /// Scans `~/.claude/teams/` for directories and checks if the corresponding
    /// factory daemon socket (`~/.cas/factory-{name}.sock`) still exists. If the
    /// socket is gone, the daemon crashed without cleaning up and the team
    /// directory is safe to remove.
    ///
    /// Called once at daemon startup to clean up after previous crashes.
    pub fn cleanup_orphans() {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let teams_root = home.join(".claude").join("teams");

        let entries = match std::fs::read_dir(&teams_root) {
            Ok(entries) => entries,
            Err(_) => return, // No teams directory at all
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            // Check if the factory daemon socket still exists
            let sock_path = home.join(".cas").join(format!("factory-{dir_name}.sock"));

            if !sock_path.exists() {
                tracing::info!(
                    "Removing orphaned teams directory {:?} (no daemon socket)",
                    path
                );
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    tracing::warn!("Failed to remove orphaned teams dir {:?}: {}", path, e);
                }
            }
        }
    }

    /// Build a `cas_mux::TeamsSpawnConfig` for spawning a new agent with native teams flags.
    ///
    /// Used for dynamically-added workers (agents added after the initial
    /// `init_team_config` call). Eagerly writes the per-worker settings file
    /// into `self.teams_dir` so the spawned `claude` invocation's
    /// `--settings <path>` resolves at PTY start — same invariant as the
    /// eager-write path in [`Self::build_configs_for_mux`].
    pub fn spawn_config_for(
        &self,
        name: &str,
        agent_type: &str,
        color: &str,
        parent_session_id: Option<&str>,
    ) -> cas_mux::TeamsSpawnConfig {
        let worker_settings_path = self
            .teams_dir
            .join(format!("{name}-settings.json"));
        if let Err(e) = Self::write_worker_settings_to(&worker_settings_path) {
            tracing::warn!(
                "Failed to write worker settings for {} at {:?}: {}",
                name,
                worker_settings_path,
                e
            );
        }

        cas_mux::TeamsSpawnConfig {
            team_name: self.team_name.clone(),
            agent_id: self.agent_id_for(name),
            agent_name: name.to_string(),
            agent_color: color.to_string(),
            agent_type: agent_type.to_string(),
            parent_session_id: parent_session_id.map(|s| s.to_string()),
            lead_session_id: None,
            settings_path: Some(worker_settings_path.to_string_lossy().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point the manager at a temp directory instead of `~/.claude/teams/...`
    /// so the test doesn't collide with real factory sessions. We keep the
    /// production constructor in place and just override the internal paths;
    /// that also exercises the real file layout the supervisor CLI sees.
    fn manager_in(tmp: &std::path::Path, name: &str) -> TeamsManager {
        let teams_dir = tmp.join(".claude").join("teams").join(name);
        let inboxes_dir = teams_dir.join("inboxes");
        TeamsManager {
            team_name: name.to_string(),
            teams_dir,
            inboxes_dir,
        }
    }

    /// `supervisor_settings_contents()` must cover every tool family whose
    /// approvals would otherwise hang on the phantom `team-lead` mailbox.
    /// Workers learned the same lesson (Read/Glob/Grep were the original
    /// screenshot's blockers), and the supervisor hits the same ops while
    /// auditing its own `.claude/settings.json`.
    #[test]
    fn supervisor_settings_contents_covers_expected_tools() {
        let body = TeamsManager::supervisor_settings_contents();
        let allow = body
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .expect("permissions.allow present");
        let names: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
        for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"] {
            assert!(
                names.contains(&tool),
                "supervisor allowlist must include {tool}, got {names:?}"
            );
        }
    }

    /// Worker allowlist must cover the same filesystem tool families so
    /// every Write/Edit/Read/Glob/Grep/Bash op auto-approves instead of
    /// escalating to the phantom `team-lead`.
    #[test]
    fn worker_settings_contents_covers_expected_tools() {
        let body = TeamsManager::worker_settings_contents();
        let allow = body
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .expect("permissions.allow present");
        let names: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
        for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"] {
            assert!(
                names.contains(&tool),
                "worker allowlist must include {tool}, got {names:?}"
            );
        }
    }

    /// Both per-role settings bodies must wire the factory auto-approve
    /// hooks. Without these entries, `cas hook PreToolUse` and
    /// `cas hook PermissionRequest` are never invoked and the team-mode
    /// UG9 escalation self-deadlocks on every permission gate (the bug that
    /// regressed when `715891c` stripped project-level hooks expecting them
    /// to live in per-member settings, but the per-member settings writer
    /// never had them).
    ///
    /// cas-9a60: emitters use exec-form `"args"` now that
    /// anthropics/claude-code#58441 is fixed in CC ≥ 2.1.142.
    #[test]
    fn settings_contents_wire_factory_auto_approve_hooks() {
        for (role, body) in [
            ("supervisor", TeamsManager::supervisor_settings_contents()),
            ("worker", TeamsManager::worker_settings_contents()),
        ] {
            let hooks = body
                .get("hooks")
                .unwrap_or_else(|| panic!("{role} settings missing `hooks` block: {body}"));

            let pre = hooks
                .get("PreToolUse")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .unwrap_or_else(|| panic!("{role} hooks missing PreToolUse entry: {hooks}"));
            let pre_args: Vec<&str> = pre
                .get("hooks")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|h| h.get("args"))
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_else(|| panic!("{role} PreToolUse missing exec-form args: {pre}"));
            assert_eq!(
                pre_args,
                vec!["cas", "hook", "PreToolUse"],
                "{role} PreToolUse must invoke `cas hook PreToolUse` via exec-form args (cas-9a60)"
            );
            let matcher = pre
                .get("matcher")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{role} PreToolUse missing matcher: {pre}"));
            for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"] {
                assert!(
                    matcher.contains(tool),
                    "{role} PreToolUse matcher must cover {tool}, got {matcher:?}"
                );
            }

            let perm = hooks
                .get("PermissionRequest")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .unwrap_or_else(|| panic!("{role} hooks missing PermissionRequest entry: {hooks}"));
            let perm_args: Vec<&str> = perm
                .get("hooks")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|h| h.get("args"))
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_else(|| panic!("{role} PermissionRequest missing exec-form args: {perm}"));
            assert_eq!(
                perm_args,
                vec!["cas", "hook", "PermissionRequest"],
                "{role} PermissionRequest must invoke `cas hook PermissionRequest` via exec-form args (cas-9a60)"
            );
        }
    }

    /// `build_configs_for_mux` must populate `settings_path` on every worker
    /// entry (not just the supervisor). Before this fix workers got `None` and
    /// every filesystem tool call escalated to `team-lead`, a mailbox that
    /// doesn't exist, and hung forever.
    #[test]
    fn build_configs_for_mux_sets_settings_path_on_every_worker() {
        // Use unique session name so parallel test runs don't race.
        let uniq = format!(
            "worker-allowlist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let (configs, _lead) = TeamsManager::build_configs_for_mux(
            &uniq,
            "supervisor",
            &["worker-a".to_string(), "worker-b".to_string()],
        );

        for worker in ["worker-a", "worker-b"] {
            let w = configs.get(worker).expect("worker config");
            let path = w
                .settings_path
                .as_ref()
                .unwrap_or_else(|| panic!("worker {worker} must carry settings_path"));
            assert!(
                path.ends_with(&format!("{worker}-settings.json")),
                "worker {worker} settings_path should end with {worker}-settings.json, got {path}"
            );
            assert!(
                path.contains(&uniq),
                "worker {worker} settings_path must live under session dir, got {path}"
            );
        }

        // Cleanup
        let root = TeamsManager::supervisor_settings_path_for(&uniq);
        if let Some(dir) = root.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Worker settings files must be written to disk at the moment
    /// `build_configs_for_mux` returns — before any worker PTY is spawned.
    /// A missing file at spawn time means `claude --settings <path>` silently
    /// falls back to the stock allowlist, recreating the hang.
    #[test]
    fn build_configs_for_mux_writes_worker_settings_files_eagerly() {
        let uniq = format!(
            "worker-eager-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let worker_a_path = TeamsManager::worker_settings_path_for(&uniq, "worker-a");
        let worker_b_path = TeamsManager::worker_settings_path_for(&uniq, "worker-b");
        assert!(!worker_a_path.exists(), "precondition: worker-a settings file absent");
        assert!(!worker_b_path.exists(), "precondition: worker-b settings file absent");

        let _ = TeamsManager::build_configs_for_mux(
            &uniq,
            "supervisor",
            &["worker-a".to_string(), "worker-b".to_string()],
        );

        assert!(
            worker_a_path.exists(),
            "worker-a settings must be written eagerly at {worker_a_path:?}"
        );
        assert!(
            worker_b_path.exists(),
            "worker-b settings must be written eagerly at {worker_b_path:?}"
        );

        // Cleanup
        if let Some(dir) = worker_a_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Dynamically-spawned workers go through `spawn_config_for` instead of
    /// `build_configs_for_mux`; that path must also write + populate
    /// `settings_path` or the deadlock recurs for runtime-added workers.
    #[test]
    fn spawn_config_for_writes_worker_settings_and_populates_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let uniq = "dynamic-worker-test";
        let tm = manager_in(tmp.path(), uniq);

        let config =
            tm.spawn_config_for("late-joiner", "general-purpose", "blue", Some("lead-xyz"));

        let path = config
            .settings_path
            .as_ref()
            .expect("spawn_config_for must populate settings_path for workers");
        assert!(path.ends_with("late-joiner-settings.json"));

        let on_disk = std::path::PathBuf::from(path);
        assert!(
            on_disk.exists(),
            "spawn_config_for must eagerly write the worker settings file at {on_disk:?}"
        );
    }

    #[test]
    fn init_team_config_writes_supervisor_settings_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tm = manager_in(tmp.path(), "deadlock-test-team");
        let (_configs, lead_session_id) =
            TeamsManager::build_configs_for_mux("deadlock-test-team", "supervisor", &[]);

        tm.init_team_config(&[], tmp.path(), &std::collections::HashMap::new(), &lead_session_id)
            .expect("init");

        let settings_path = tm.teams_dir.join("supervisor-settings.json");
        assert!(
            settings_path.exists(),
            "supervisor-settings.json should be written next to config.json"
        );

        let body = std::fs::read_to_string(&settings_path).expect("read settings");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        let allow = parsed
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .expect("permissions.allow array present");

        let names: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
        // Every filesystem tool family the routing deadlock is observed on.
        // The list was expanded from the original 4 (Write/Edit/Bash/
        // NotebookEdit) after cas-e15d: Read/Glob/Grep were also hanging
        // when the supervisor audited `.claude/settings.json`.
        for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"] {
            assert!(
                names.contains(&tool),
                "supervisor allow must include {tool}, got {names:?}"
            );
        }
    }

    #[test]
    fn build_configs_for_mux_sets_supervisor_settings_path() {
        let uniq = format!(
            "routing-supervisor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let (configs, _lead_session_id) = TeamsManager::build_configs_for_mux(
            &uniq,
            "supervisor",
            &["worker-1".to_string(), "worker-2".to_string()],
        );

        let sup = configs.get("supervisor").expect("supervisor config");
        let path = sup
            .settings_path
            .as_ref()
            .expect("supervisor must carry a settings_path so --settings is emitted");
        assert!(
            path.ends_with("supervisor-settings.json"),
            "settings_path should point at supervisor-settings.json, got {path}"
        );
        assert!(
            path.contains(&uniq),
            "settings_path should live under the session's team dir, got {path}"
        );

        // Cleanup
        let root = TeamsManager::supervisor_settings_path_for(&uniq);
        if let Some(dir) = root.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Core invariant: the supervisor settings file must exist on disk by the
    /// time `build_configs_for_mux` returns, because that's the latest moment
    /// before the factory calls `FactoryApp::new` → `Mux::factory` and spawns
    /// the supervisor PTY with `--settings <path>`. A missing file at spawn
    /// time means `claude` silently falls back to the stock allowlist and the
    /// deadlock recurs.
    #[test]
    fn build_configs_for_mux_writes_settings_file_eagerly() {
        // Use a unique session name so parallel test runs don't race each
        // other on the same path in $HOME/.claude/teams/. The test cleans
        // up after itself at the end.
        let uniq = format!(
            "deadlock-eager-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let expected_path = TeamsManager::supervisor_settings_path_for(&uniq);
        assert!(
            !expected_path.exists(),
            "precondition: settings file must not exist before build_configs_for_mux"
        );

        let (_configs, _lead_id) =
            TeamsManager::build_configs_for_mux(&uniq, "supervisor", &[]);

        assert!(
            expected_path.exists(),
            "build_configs_for_mux must write supervisor-settings.json eagerly; \
             missing at {expected_path:?} would cause --settings to resolve to \
             nothing when the supervisor PTY spawns"
        );

        // Cleanup
        if let Some(dir) = expected_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Cross-refresh replay: identical (from, text) writes within
    /// `INBOX_DEDUP_WINDOW` must no-op, dropping the duplicate before it
    /// reaches the worker. This is the core regression guard for cas-7f57
    /// — workers observed "You have been assigned cas-X" messages replayed
    /// minutes apart for tasks that were already Closed.
    #[test]
    fn write_to_inbox_dedups_identical_messages_within_window() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t1");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();
        mgr.ensure_inbox("swift-fox").unwrap();

        let msg = "You have been assigned cas-7f57\nTask: dup guard";
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, msg, None, None)
            .unwrap();
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, msg, None, None)
            .unwrap();
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, msg, None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("swift-fox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inbox.len(),
            1,
            "identical within-window writes must dedupe down to one entry; inbox={inbox:?}"
        );

        // A genuinely different payload still gets through.
        mgr.write_to_inbox(
            "swift-fox",
            DIRECTOR_AGENT_NAME,
            "Worker is idle — pick up a task",
            None,
            None,
        )
        .unwrap();
        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("swift-fox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 2, "distinct payload must not be deduped");
    }

    /// Writes from different senders with the same text are independent —
    /// dedup keys on (from, text). Guards against overly aggressive
    /// collapse that would swallow legitimate cross-sender broadcasts.
    #[test]
    fn write_to_inbox_dedup_is_per_sender() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t2");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();
        mgr.ensure_inbox("swift-fox").unwrap();

        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "ping", None, None)
            .unwrap();
        mgr.write_to_inbox("swift-fox", "supervisor", "ping", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("swift-fox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inbox.len(),
            2,
            "same text from different senders must both be retained; inbox={inbox:?}"
        );
    }

    /// A duplicate write beyond `INBOX_DEDUP_WINDOW` must pass through
    /// and append — dedup is time-bounded, not permanent. Guards against
    /// off-by-one sign flips on the cutoff comparison.
    #[test]
    fn write_to_inbox_does_not_dedup_past_window() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_expire");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        // Seed inbox with a 15-minute-old duplicate (beyond 10-min window).
        let old_ts = (chrono::Utc::now() - chrono::Duration::minutes(15))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seeded = vec![InboxMessage {
            from: DIRECTOR_AGENT_NAME.to_string(),
            text: "ping".to_string(),
            summary: Some("ping".to_string()),
            timestamp: old_ts,
            color: "green".to_string(),
            // Must be marked read or retention will keep it for
            // unrelated reasons; we want dedup to be the only variable.
            read: true,
        }];
        let inbox_path = mgr.inboxes_dir.join("swift-fox.json");
        std::fs::write(&inbox_path, serde_json::to_string_pretty(&seeded).unwrap())
            .unwrap();

        // Fresh write with identical (from, text). Must pass through.
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "ping", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(&inbox_path).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inbox.len(),
            2,
            "15-minute-old duplicate is beyond dedup window and must not suppress a fresh write"
        );
    }

    /// Unread messages (`read: false`) survive the retention sweep even
    /// when older than `INBOX_RETENTION`. Guards the cas-7f57 adversarial
    /// P1 finding: a supervisor recovery prompt to a wedged worker must
    /// not silently evaporate after 2h.
    #[test]
    fn write_to_inbox_retention_preserves_unread_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_unread");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        // Seed a 3h-old UNREAD message (beyond 2h retention). Also seed
        // a 3h-old READ message to prove the distinction.
        let stale_ts = (chrono::Utc::now() - chrono::Duration::hours(3))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seeded = vec![
            InboxMessage {
                from: "supervisor".to_string(),
                text: "unblock yourself".to_string(),
                summary: Some("unblock yourself".to_string()),
                timestamp: stale_ts.clone(),
                color: "green".to_string(),
                read: false,
            },
            InboxMessage {
                from: DIRECTOR_AGENT_NAME.to_string(),
                text: "already-acked nag".to_string(),
                summary: Some("already-acked nag".to_string()),
                timestamp: stale_ts,
                color: "green".to_string(),
                read: true,
            },
        ];
        let inbox_path = mgr.inboxes_dir.join("swift-fox.json");
        std::fs::write(&inbox_path, serde_json::to_string_pretty(&seeded).unwrap())
            .unwrap();

        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "fresh", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(&inbox_path).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 2, "inbox should retain unread + fresh, got {inbox:?}");
        assert!(
            inbox.iter().any(|m| m.text == "unblock yourself" && !m.read),
            "unread supervisor recovery message must survive retention"
        );
        assert!(
            !inbox.iter().any(|m| m.text == "already-acked nag"),
            "stale read message must still be pruned"
        );
        assert!(
            inbox.iter().any(|m| m.text == "fresh"),
            "fresh write should have landed"
        );
    }

    /// Retention sweep: messages older than `INBOX_RETENTION` are dropped
    /// on every write so the inbox file cannot grow unbounded across
    /// sessions. Simulated by seeding a message with an old timestamp and
    /// then writing a fresh one.
    #[test]
    fn write_to_inbox_prunes_messages_older_than_retention() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t3");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        // Seed an inbox file with a stale message (3h ago, beyond the 2h
        // retention window).
        let stale_ts = (chrono::Utc::now() - chrono::Duration::hours(3))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seeded = vec![InboxMessage {
            from: DIRECTOR_AGENT_NAME.to_string(),
            text: "ancient history".to_string(),
            summary: Some("ancient history".to_string()),
            timestamp: stale_ts,
            color: "green".to_string(),
            // Must be marked read — unread messages are preserved
            // regardless of age by design.
            read: true,
        }];
        let inbox_path = mgr.inboxes_dir.join("swift-fox.json");
        std::fs::write(&inbox_path, serde_json::to_string_pretty(&seeded).unwrap()).unwrap();

        // One fresh write — the stale message should be swept on the same
        // lock pass.
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "fresh", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(&inbox_path).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].text, "fresh");
    }
}
