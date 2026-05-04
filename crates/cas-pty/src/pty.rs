//! PTY management using portable-pty
//!
//! Provides a wrapper around portable-pty with:
//! - Async read/write operations
//! - Raw byte output (terminal parsing done by ghostty_vt)
//! - Resize support

use crate::error::{Error, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

/// Instructions injected into Codex supervisor agents via `--config developer_instructions`.
const CODEX_SUPERVISOR_INSTRUCTIONS: &str = "You are the CAS Factory Supervisor. Coordinate only: plan epics, assign tasks, monitor progress, review/merge. Never implement tasks. Use skills cas-supervisor and cas-codex-supervisor-checklist. Use MCP tools explicitly; no /cas-start, /cas-context, or /cas-end.";

/// Instructions injected into Codex worker agents via `--config developer_instructions`.
const CODEX_WORKER_INSTRUCTIONS: &str = "You are a CAS Factory Worker. Always use CAS MCP tools for task lifecycle and coordination. On startup run `mcp__cs__coordination action=session_start name=<worker-name> agent_type=worker` then `mcp__cs__coordination action=whoami`, then run `mcp__cs__task action=mine`. For assigned tasks run `mcp__cs__task action=show id=<task-id>` then `mcp__cs__task action=start id=<task-id>` before coding. Add progress notes frequently using `mcp__cs__task action=notes id=<task-id> note_type=progress notes=\"...\"`. For blockers, add blocker note, set `status=blocked`, and message supervisor via `mcp__cs__coordination action=message target=supervisor message=\"...\"`. When implementation is complete, close with `mcp__cs__task action=close id=<task-id> reason=\"...\"`. If close returns verification-required guidance, immediately ask supervisor to verify/close on your behalf. Do not use /cas-start, /cas-context, or /cas-end. Stay within assigned task scope.";

/// Prefix for the Codex worker startup prompt. The worker name is appended at runtime.
const CODEX_WORKER_STARTUP_PREFIX: &str = "I'm initiating CAS worker startup now: register this worker session, confirm identity, check assigned tasks, then start any assigned task with a progress note.\n1) Run mcp__cs__coordination action=session_start name=";

/// Configuration for spawning a PTY
#[derive(Debug, Clone)]
pub struct PtyConfig {
    /// Command to run (e.g., "claude")
    pub command: String,
    /// Arguments for the command
    pub args: Vec<String>,
    /// Working directory
    pub cwd: Option<PathBuf>,
    /// Environment variables to set
    pub env: Vec<(String, String)>,
    /// Initial terminal size
    pub rows: u16,
    pub cols: u16,
}

/// Configuration for spawning an agent with native Claude Code Agent Teams flags.
#[derive(Debug, Clone)]
pub struct TeamsSpawnConfig {
    /// Team name (factory session name)
    pub team_name: String,
    /// Agent ID (e.g., "worker-1@session-name")
    pub agent_id: String,
    /// Agent display name
    pub agent_name: String,
    /// Agent color for UI
    pub agent_color: String,
    /// Agent type (e.g., "team-lead", "general-purpose")
    pub agent_type: String,
    /// Parent session ID for analytics correlation (workers only)
    pub parent_session_id: Option<String>,
    /// Lead session ID — set for the team lead so --session-id matches leadSessionId
    pub lead_session_id: Option<String>,
    /// Optional path to a settings JSON file passed via `--settings <path>`.
    ///
    /// Populated for both the supervisor (`supervisor-settings.json`) and for
    /// every worker (`{worker-name}-settings.json`) so filesystem tool calls
    /// auto-approve from the per-role allowlist instead of escalating through
    /// the team-approval channel. Workers without this file hang on the
    /// phantom `team-lead` mailbox because Claude Code's harness misreads
    /// `agentType="team-lead"` as the lead's display name (upstream bug);
    /// shipping the allowlist eliminates the trigger even while that misread
    /// remains unfixed. See `cas-cli/src/ui/factory/daemon/runtime/teams.rs`
    /// (`supervisor_settings_contents` / `worker_settings_contents`) for the
    /// shape of each file.
    pub settings_path: Option<String>,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            command: "bash".to_string(),
            args: vec![],
            cwd: None,
            env: vec![],
            rows: 24,
            cols: 80,
        }
    }
}

impl PtyConfig {
    /// Create config for a Claude CLI instance
    ///
    /// # Arguments
    /// * `name` - Agent name
    /// * `role` - Agent role (e.g., "worker", "supervisor")
    /// * `cwd` - Working directory for the agent
    /// * `cas_root` - Optional path to the .cas directory. If provided, sets CAS_ROOT env var
    ///   so workers in clones can access the main repo's CAS state.
    /// * `supervisor_name` - For workers, the name of their supervisor (enables `target: supervisor`)
    #[allow(clippy::too_many_arguments)]
    pub fn claude(
        name: &str,
        role: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: Option<&str>,
        factory_worker_cli: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
    ) -> Self {
        // Use the lead_session_id for the team lead so leadSessionId in the
        // team config matches the supervisor's --session-id. Without this,
        // Claude Code thinks it's not the leader and won't process inbox.
        let session_id = teams
            .and_then(|t| t.lead_session_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let mut env = vec![
            ("CAS_AGENT_NAME".to_string(), name.to_string()),
            ("CAS_AGENT_ROLE".to_string(), role.to_string()),
            // Mark this process as running inside a factory session.
            // Read by pre_tool jail, close_ops, mcp server, and task update
            // to branch factory-vs-standalone behavior. Without this, the
            // is_factory_worker check in pre_tool.rs fails (it requires both
            // CAS_AGENT_ROLE=worker AND CAS_FACTORY_MODE), so workers get
            // jailed on every verification-pending task.
            ("CAS_FACTORY_MODE".to_string(), "1".to_string()),
            // Provide session ID so CAS MCP server can self-register without hooks
            ("CAS_SESSION_ID".to_string(), session_id.clone()),
            // Set clone path so subagents know the worktree directory
            (
                "CAS_CLONE_PATH".to_string(),
                cwd.to_string_lossy().to_string(),
            ),
            // Suppress interactive prompts, telemetry, and updates for factory agents
            (
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
                "1".to_string(),
            ),
            ("DISABLE_AUTOUPDATER".to_string(), "1".to_string()),
            ("DISABLE_COST_WARNINGS".to_string(), "1".to_string()),
            (
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE".to_string(),
                "1".to_string(),
            ),
            ("IS_DEMO".to_string(), "true".to_string()),
        ];

        // Set CAS_ROOT env var if provided (enables workers in clones to use main's .cas)
        if let Some(root) = cas_root {
            env.push(("CAS_ROOT".to_string(), root.to_string_lossy().to_string()));
        }

        // Set supervisor name for workers (enables `target: supervisor` in message action)
        if let Some(sup) = supervisor_name {
            env.push(("CAS_SUPERVISOR_NAME".to_string(), sup.to_string()));
        }
        if let Some(worker_cli) = factory_worker_cli {
            env.push(("CAS_FACTORY_WORKER_CLI".to_string(), worker_cli.to_string()));
        }

        // cas-0bf4: cap cargo parallelism inside factory worker processes
        // so a 4-worker factory doesn't stack `num_cpus`-way rustc jobs
        // per worker and wedge the host via scheduler starvation
        // (cas-4513 Claude Code JS crash-screen symptom). Emitted only
        // for role="worker"; supervisor stays uncapped.
        push_worker_cargo_env(&mut env, role);

        // Enable native Agent Teams for inter-agent messaging
        if teams.is_some() {
            env.push((
                "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(),
                "1".to_string(),
            ));
        }

        let mut args = vec![
            "--dangerously-skip-permissions".to_string(),
            "--session-id".to_string(),
            session_id,
        ];
        if let Some(m) = model {
            args.push("--model".to_string());
            args.push(m.to_string());
        }
        // Supervisors need deeper reasoning for planning/coordination;
        // workers execute well-defined tasks where high effort suffices.
        // Config-provided effort takes precedence; role-based defaults preserve
        // backward compatibility when no config value is set.
        let resolved_effort =
            effort.unwrap_or(if role == "supervisor" { "xhigh" } else { "high" });
        args.push("--effort".to_string());
        args.push(resolved_effort.to_string());

        // Add native Agent Teams CLI flags.
        // All agents (including the supervisor) get --teammate-mode tmux
        // so Claude Code activates inbox polling for everyone.
        if let Some(t) = teams {
            args.push("--team-name".to_string());
            args.push(t.team_name.clone());
            args.push("--agent-id".to_string());
            args.push(t.agent_id.clone());
            args.push("--agent-name".to_string());
            args.push(t.agent_name.clone());
            args.push("--agent-color".to_string());
            args.push(t.agent_color.clone());
            args.push("--agent-type".to_string());
            args.push(t.agent_type.clone());
            args.push("--teammate-mode".to_string());
            args.push("tmux".to_string());
            if let Some(ref parent_id) = t.parent_session_id {
                args.push("--parent-session-id".to_string());
                args.push(parent_id.clone());
            }
            // Per-role settings file — both supervisor and workers ship a
            // `permissions.allow` list via `--settings` so Read/Write/Edit/
            // Glob/Grep/Bash/NotebookEdit auto-approve instead of escalating
            // through team-approval routing (the phantom `team-lead` hang).
            // If the caller leaves `settings_path` as None (CLI usage,
            // standalone claude invocations, or tests that deliberately
            // opt out), no flag is emitted — that's a valid fallback.
            if let Some(ref settings_path) = t.settings_path {
                args.push("--settings".to_string());
                args.push(settings_path.clone());
            }
        }

        // cas-0bf4: optionally lower the worker's scheduling priority so
        // the supervisor's Claude Code event loop wins scheduler fights.
        // Only fires for role="worker" when `CAS_FACTORY_NICE_WORKER=1`
        // is set by the supervisor-side factory config bridge.
        let (command, args) = maybe_wrap_with_nice("claude", args, role);

        Self {
            command,
            args,
            cwd: Some(cwd),
            env,
            rows: 24,
            cols: 80,
        }
    }

    /// Create config for a Codex CLI instance
    ///
    /// # Arguments
    /// * `name` - Agent name
    /// * `role` - Agent role (e.g., "worker", "supervisor")
    /// * `cwd` - Working directory for the agent
    /// * `cas_root` - Optional path to the .cas directory. If provided, sets CAS_ROOT env var
    /// * `supervisor_name` - For workers, the name of their supervisor (enables `target: supervisor`)
    #[allow(clippy::too_many_arguments)]
    pub fn codex(
        name: &str,
        role: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: Option<&str>,
        factory_worker_cli: Option<&str>,
        model: Option<&str>,
        _effort: Option<&str>,
        _teams: Option<&TeamsSpawnConfig>,
    ) -> Self {
        // Native Agent Teams is Claude Code-only; Codex CLI does not support it.
        let session_id = uuid::Uuid::new_v4().to_string();

        let mut env = vec![
            ("CAS_AGENT_NAME".to_string(), name.to_string()),
            ("CAS_AGENT_ROLE".to_string(), role.to_string()),
            // Mark this process as running inside a factory session.
            // See equivalent comment in `claude()` above — without this the
            // pre_tool verification-jail exemption for factory workers does
            // not fire and workers get jailed on every pending task.
            ("CAS_FACTORY_MODE".to_string(), "1".to_string()),
            // Provide session ID so CAS MCP server can self-register without hooks
            ("CAS_SESSION_ID".to_string(), session_id),
            (
                "CAS_CLONE_PATH".to_string(),
                cwd.to_string_lossy().to_string(),
            ),
            // Suppress interactive prompts, telemetry, and updates for factory agents
            (
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
                "1".to_string(),
            ),
            ("DISABLE_AUTOUPDATER".to_string(), "1".to_string()),
            ("DISABLE_COST_WARNINGS".to_string(), "1".to_string()),
            (
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE".to_string(),
                "1".to_string(),
            ),
            ("IS_DEMO".to_string(), "true".to_string()),
        ];

        if let Ok(term) = std::env::var("TERM")
            && term.contains("ghostty")
        {
            env.push(("TERM".to_string(), "xterm-256color".to_string()));
        }

        if let Some(root) = cas_root {
            env.push(("CAS_ROOT".to_string(), root.to_string_lossy().to_string()));
        }

        if let Some(sup) = supervisor_name {
            env.push(("CAS_SUPERVISOR_NAME".to_string(), sup.to_string()));
        }
        if let Some(worker_cli) = factory_worker_cli {
            env.push(("CAS_FACTORY_WORKER_CLI".to_string(), worker_cli.to_string()));
        }

        // cas-0bf4: see equivalent comment in `claude()`.
        push_worker_cargo_env(&mut env, role);

        let mut args = vec!["--yolo".to_string(), "--no-alt-screen".to_string()];
        if let Some(m) = model {
            args.push("--model".to_string());
            args.push(m.to_string());
        }

        if role == "supervisor" {
            let escaped = CODEX_SUPERVISOR_INSTRUCTIONS.replace('"', "\\\"");
            args.push("--config".to_string());
            args.push(format!("developer_instructions=\"{escaped}\""));
        } else if role == "worker" {
            let escaped = CODEX_WORKER_INSTRUCTIONS.replace('"', "\\\"");
            args.push("--config".to_string());
            args.push(format!("developer_instructions=\"{escaped}\""));

            // Pass startup workflow as initial prompt arg so Codex executes it immediately.
            // This is more reliable than post-spawn typed injection, which can leave text
            // in the composer without submitting in some startup timing windows.
            let startup_prompt = format!(
                "{CODEX_WORKER_STARTUP_PREFIX}{name} agent_type=worker\n\
                 2) Run mcp__cs__coordination action=whoami\n\
                 3) Run mcp__cs__task action=mine\n\
                 4) If tasks are assigned: show/start each task and add a progress note\n\
                 5) If no tasks are assigned: send mcp__cs__coordination action=message target=supervisor confirming ready state\n\
                 6) Do NOT message target=cas. Use target=supervisor."
            );
            args.push(startup_prompt);
        }

        // cas-0bf4: see equivalent comment in `claude()`.
        let (command, args) = maybe_wrap_with_nice("codex", args, role);

        Self {
            command,
            args,
            cwd: Some(cwd),
            env,
            rows: 24,
            cols: 80,
        }
    }
}

/// Expected number of concurrent factory workers the CPU is being
/// shared among when auto-computing `CARGO_BUILD_JOBS`. On a 16-thread
/// dev box (soundwave, reference host for cas-4513 + cas-0bf4 evidence)
/// this divides the CPU budget into 4 × 4-thread slices, which kept the
/// host below scheduler saturation in the sessions where we observed
/// the Claude Code JS crash-screen wedges.
///
/// Override the assumption by setting `CAS_FACTORY_CARGO_BUILD_JOBS`
/// explicitly — e.g., a supervisor running 8 workers on a 16-thread
/// host should export `CAS_FACTORY_CARGO_BUILD_JOBS=2`.
const DEFAULT_WORKER_CONCURRENCY_ASSUMPTION: usize = 4;

/// Compute the `CARGO_BUILD_JOBS` value to export into a worker's env.
///
/// Precedence (first match wins):
///   1. `CAS_FACTORY_CARGO_BUILD_JOBS` env — set by the supervisor-side
///      factory config bridge from `factory.cargo_build_jobs` config.
///      Empty value or literal `"auto"` means "fall through to 2–4".
///   2. Auto-compute: `max(2, available_parallelism() / DEFAULT_WORKER_CONCURRENCY_ASSUMPTION)`.
///
/// Returns `None` only when auto-compute fails to read CPU topology,
/// which should be vanishingly rare. In that case we do NOT set
/// `CARGO_BUILD_JOBS` — cargo's own default (= num_cpus) then applies
/// and the cap is a no-op rather than misleading.
fn cargo_build_jobs_for_worker() -> Option<String> {
    if let Ok(explicit) = std::env::var("CAS_FACTORY_CARGO_BUILD_JOBS") {
        let trimmed = explicit.trim();
        // Case-insensitive `"auto"` falls through to the computed cap so
        // users who write `Auto`/`AUTO` in config don't silently defeat
        // the mitigation by shipping a literal non-integer value into
        // `CARGO_BUILD_JOBS`.
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("auto") {
            return Some(trimmed.to_string());
        }
    }
    let cores = std::thread::available_parallelism().ok()?.get();
    let capped = std::cmp::max(2, cores / DEFAULT_WORKER_CONCURRENCY_ASSUMPTION);
    Some(capped.to_string())
}

/// Push the `CARGO_BUILD_JOBS` env entry into `env` when `role == "worker"`.
/// Extracted from `PtyConfig::{claude,codex}` to remove the duplicated
/// block those two call sites used to carry verbatim (cas-0bf4).
fn push_worker_cargo_env(env: &mut Vec<(String, String)>, role: &str) {
    if role != "worker" {
        return;
    }
    if let Some(cargo_jobs) = cargo_build_jobs_for_worker() {
        env.push(("CARGO_BUILD_JOBS".to_string(), cargo_jobs));
    }
}

/// If `CAS_FACTORY_NICE_WORKER=1` is set in the supervisor's env and
/// `role == "worker"`, wrap the spawn command in `nice -n 10` so the
/// worker's process tree (including cargo-driven rustc jobs) runs at
/// a lower scheduling priority than the supervisor. Supervisor panes
/// stay at nice 0 and therefore win CPU-contention fights, which keeps
/// the factory steerable when workers start cargo-storming (cas-0bf4).
///
/// Non-worker roles and sessions without the sentinel env are passed
/// through unchanged. `nice` must be on PATH (standard on every Linux
/// and macOS host CAS supports); if it isn't, the worker will fail to
/// spawn with a clear "nice not found" error from the PTY layer rather
/// than silently running unwrapped — that's the safer fallback.
fn maybe_wrap_with_nice(command: &str, args: Vec<String>, role: &str) -> (String, Vec<String>) {
    if role != "worker" {
        return (command.to_string(), args);
    }
    if std::env::var("CAS_FACTORY_NICE_WORKER").as_deref() != Ok("1") {
        return (command.to_string(), args);
    }
    // Default niceness increment is 10; honour CAS_FACTORY_NICE_LEVEL
    // for power users who want a harder or softer cap. Parse as i32 so
    // a typo like `CAS_FACTORY_NICE_LEVEL=high` cannot propagate to
    // `nice -n high claude ...` and kill every worker spawn with an
    // opaque PTY error — we quietly fall back to the default 10.
    let level = std::env::var("CAS_FACTORY_NICE_LEVEL")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "10".to_string());
    let mut new_args = Vec::with_capacity(args.len() + 3);
    new_args.push("-n".to_string());
    new_args.push(level);
    new_args.push(command.to_string());
    new_args.extend(args);
    ("nice".to_string(), new_args)
}

/// Events emitted by a PTY
#[derive(Debug, Clone)]
pub enum PtyEvent {
    /// Terminal output (raw bytes - parsing done by ghostty_vt)
    Output(Vec<u8>),
    /// Process exited
    Exited(Option<i32>),
    /// Error occurred
    Error(String),
}

/// A running PTY process
pub struct Pty {
    /// Unique identifier
    id: String,
    /// Writer handle for sending input
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Channel for receiving raw output
    event_rx: mpsc::Receiver<PtyEvent>,
    /// Handle to the reader task
    _reader_handle: std::thread::JoinHandle<()>,
    /// Child process handle
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Master PTY (keep alive)
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Whether this PTY is running Codex CLI
    is_codex: bool,
}

impl Pty {
    /// Spawn a new PTY with the given configuration
    pub fn spawn(id: impl Into<String>, config: PtyConfig) -> Result<Self> {
        let id = id.into();
        let is_codex = config.command == "codex";

        // Create PTY system and open a PTY pair
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.rows,
                cols: config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::pty(format!("Failed to open PTY: {e}")))?;

        // Build command
        let mut cmd = CommandBuilder::new(&config.command);
        cmd.args(&config.args);

        if let Some(cwd) = &config.cwd {
            cmd.cwd(cwd);
        }

        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        // Strip CLAUDECODE to prevent nested-session detection in spawned Claude CLI
        cmd.env_remove("CLAUDECODE");

        // Spawn the child process
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::pty(format!("Failed to spawn command: {e}")))?;

        // Drop slave - the child process owns it now
        drop(pair.slave);

        // Get reader and writer
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::pty(format!("Failed to clone reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| Error::pty(format!("Failed to get writer: {e}")))?;

        let writer = Arc::new(Mutex::new(writer));

        if is_codex {
            let writer = Arc::clone(&writer);
            tokio::spawn(async move {
                for _ in 0..10 {
                    let mut locked = writer.lock().await;
                    let _ = locked.write_all(b"\x1b[1;1R");
                    let _ = locked.flush();
                    drop(locked);
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            });
        }

        // Create channel for events - larger buffer for multi-agent scenarios
        let (event_tx, event_rx) = mpsc::channel::<PtyEvent>(1024);

        // Spawn reader thread - sends raw bytes, no parsing
        let reader_handle = std::thread::spawn({
            let writer = Arc::clone(&writer);
            move || {
                Self::reader_loop(reader, writer, event_tx);
            }
        });

        Ok(Self {
            id,
            writer,
            event_rx,
            _reader_handle: reader_handle,
            child,
            master: pair.master,
            is_codex,
        })
    }

    /// Reader loop that forwards raw PTY output
    fn reader_loop(
        mut reader: Box<dyn Read + Send>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        event_tx: mpsc::Sender<PtyEvent>,
    ) {
        // Larger buffer for high-throughput scenarios (6 Claudes generating long responses)
        let mut buf = [0u8; 16384];
        let mut carry: Vec<u8> = Vec::new();

        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited
                    if !carry.is_empty() {
                        let _ =
                            event_tx.blocking_send(PtyEvent::Output(std::mem::take(&mut carry)));
                    }
                    let _ = event_tx.blocking_send(PtyEvent::Exited(None));
                    break;
                }
                Ok(n) => {
                    let (data, new_carry, saw_cpr) =
                        filter_cursor_position_requests(&carry, &buf[..n]);
                    carry = new_carry;

                    if saw_cpr {
                        let mut locked = writer.blocking_lock();
                        let _ = locked.write_all(b"\x1b[1;1R");
                        let _ = locked.flush();
                    }

                    if !data.is_empty() && event_tx.blocking_send(PtyEvent::Output(data)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.blocking_send(PtyEvent::Error(e.to_string()));
                    break;
                }
            }
        }
    }

    /// Get the PTY's identifier
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns true when this PTY is running Codex CLI.
    pub fn is_codex(&self) -> bool {
        self.is_codex
    }

    /// Get a clone of the writer handle (for concurrent writing)
    pub fn writer_handle(&self) -> Arc<Mutex<Box<dyn Write + Send>>> {
        self.writer.clone()
    }

    /// Write input to the PTY (for prompt injection)
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer
            .write_all(data)
            .map_err(|e| Error::pty(format!("Write failed: {e}")))?;
        writer
            .flush()
            .map_err(|e| Error::pty(format!("Flush failed: {e}")))?;
        Ok(())
    }

    /// Write a string to the PTY
    pub async fn write_str(&self, s: &str) -> Result<()> {
        self.write(s.as_bytes()).await
    }

    /// Send a line of input (appends carriage return to submit, same as Enter key)
    pub async fn send_line(&self, line: &str) -> Result<()> {
        self.write_str(&format!("{line}\r")).await
    }

    /// Receive the next event from the PTY (blocking)
    pub async fn recv(&mut self) -> Option<PtyEvent> {
        self.event_rx.recv().await
    }

    /// Try to receive an event from the PTY (non-blocking)
    pub fn try_recv(&mut self) -> Option<PtyEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Resize the PTY
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::pty(format!("Resize failed: {e}")))
    }

    /// Send Ctrl+C to the process
    pub async fn interrupt(&self) -> Result<()> {
        self.write(&[0x03]).await
    }

    /// Send Ctrl+D (EOF) to the process
    pub async fn send_eof(&self) -> Result<()> {
        self.write(&[0x04]).await
    }

    /// Kill the child process
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

fn filter_cursor_position_requests(carry: &[u8], chunk: &[u8]) -> (Vec<u8>, Vec<u8>, bool) {
    const CPR: [u8; 4] = [0x1b, 0x5b, 0x36, 0x6e]; // ESC [ 6 n
    const CPR_ALT: [u8; 5] = [0x1b, 0x5b, 0x3f, 0x36, 0x6e]; // ESC [ ? 6 n
    let max_seq = CPR_ALT.len();

    let total_len = carry.len() + chunk.len();
    if total_len == 0 {
        return (Vec::new(), Vec::new(), false);
    }

    let process_len = total_len.saturating_sub(max_seq - 1);
    let mut out = Vec::with_capacity(process_len);
    let mut i = 0usize;
    let mut saw_cpr = false;

    let byte_at = |idx: usize| -> u8 {
        if idx < carry.len() {
            carry[idx]
        } else {
            chunk[idx - carry.len()]
        }
    };

    while i < process_len {
        if i + CPR_ALT.len() <= total_len {
            let mut matches = true;
            for (j, byte) in CPR_ALT.iter().enumerate() {
                if byte_at(i + j) != *byte {
                    matches = false;
                    break;
                }
            }
            if matches {
                saw_cpr = true;
                i += CPR_ALT.len();
                continue;
            }
        }
        if i + CPR.len() <= total_len {
            let mut matches = true;
            for (j, byte) in CPR.iter().enumerate() {
                if byte_at(i + j) != *byte {
                    matches = false;
                    break;
                }
            }
            if matches {
                saw_cpr = true;
                i += CPR.len();
                continue;
            }
        }
        out.push(byte_at(i));
        i += 1;
    }

    let mut new_carry = Vec::with_capacity(total_len - process_len);
    for idx in process_len..total_len {
        new_carry.push(byte_at(idx));
    }

    (out, new_carry, saw_cpr)
}

#[cfg(test)]
mod tests {
    use crate::pty::*;
    use std::sync::{Mutex, MutexGuard};

    // cas-0bf4: module-wide serialization for any test that constructs a
    // `PtyConfig::{claude,codex}` with role="worker". Those constructors
    // now read process-wide env vars (CAS_FACTORY_CARGO_BUILD_JOBS and
    // CAS_FACTORY_NICE_WORKER) at call time; parallel tests can race if
    // one sets the sentinel while another asserts on the non-wrapped
    // command name. All worker-role PtyConfig tests must hold this
    // mutex for the duration of their body.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Lock the env mutex, clear the cas-0bf4 sentinels on entry, and
    /// clear them again on drop. Safe to use from any test that may
    /// observe or mutate those vars.
    pub(crate) struct ScopedEnv {
        _guard: MutexGuard<'static, ()>,
    }

    impl ScopedEnv {
        pub(crate) fn new() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: mutex serializes env mutation across tests.
            unsafe {
                std::env::remove_var("CAS_FACTORY_CARGO_BUILD_JOBS");
                std::env::remove_var("CAS_FACTORY_NICE_WORKER");
                std::env::remove_var("CAS_FACTORY_NICE_LEVEL");
            }
            Self { _guard: guard }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: mutex held for duration of this scope.
            unsafe {
                std::env::remove_var("CAS_FACTORY_CARGO_BUILD_JOBS");
                std::env::remove_var("CAS_FACTORY_NICE_WORKER");
                std::env::remove_var("CAS_FACTORY_NICE_LEVEL");
            }
        }
    }

    #[tokio::test]
    async fn test_pty_config_default() {
        let config = PtyConfig::default();
        assert_eq!(config.command, "bash");
        assert_eq!(config.rows, 24);
        assert_eq!(config.cols, 80);
    }

    #[tokio::test]
    async fn test_pty_config_claude() {
        let _e = ScopedEnv::new();
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // effort
            None,  // teams
        );
        assert_eq!(config.command, "claude");
        assert!(
            config
                .args
                .contains(&"--dangerously-skip-permissions".to_string())
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_AGENT_NAME" && v == "test-agent")
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_AGENT_ROLE" && v == "worker")
        );
        // No CAS_ROOT when not provided
        assert!(!config.env.iter().any(|(k, _)| k == "CAS_ROOT"));
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_cas_root() {
        let cas_root = PathBuf::from("/home/user/project/.cas");
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            Some(&cas_root),
            None,
            None,
            None,  // model
            None,  // effort
            None,  // teams
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_ROOT" && v == "/home/user/project/.cas")
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_supervisor() {
        let config = PtyConfig::claude(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            Some("test-supervisor"),
            None,
            None,  // model
            None,  // effort
            None,  // teams
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_SUPERVISOR_NAME" && v == "test-supervisor")
        );
    }

    #[tokio::test]
    async fn test_pty_config_sets_clone_path() {
        let config = PtyConfig::claude(
            "test-worker",
            "worker",
            PathBuf::from("/tmp/worktree"),
            None,
            None,
            None,
            None,  // model
            None,  // effort
            None,  // teams
        );
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CAS_CLONE_PATH" && v == "/tmp/worktree")
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_model() {
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            Some("claude-opus-4-6"),
            None,  // effort
            None,  // teams
        );
        assert!(config.args.contains(&"--model".to_string()));
        assert!(config.args.contains(&"claude-opus-4-6".to_string()));
    }

    #[tokio::test]
    async fn test_pty_config_claude_without_model() {
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // effort
            None,  // teams
        );
        assert!(!config.args.contains(&"--model".to_string()));
    }

    #[tokio::test]
    async fn test_pty_config_codex_with_model() {
        let config = PtyConfig::codex(
            "test-agent",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            Some("gpt-5.3-codex"),
            None,  // effort
            None,  // teams
        );
        assert!(config.args.contains(&"--model".to_string()));
        assert!(config.args.contains(&"gpt-5.3-codex".to_string()));
    }

    #[tokio::test]
    async fn test_pty_config_codex_worker_uses_cs_prefix() {
        let config = PtyConfig::codex(
            "test-worker",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // effort
            None,  // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("mcp__cs__"),
            "Codex worker instructions should use mcp__cs__ prefix"
        );
    }

    #[tokio::test]
    async fn test_pty_config_codex_supervisor_instructions() {
        let config = PtyConfig::codex(
            "test-supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // effort
            None,  // teams
        );
        let all_args = config.args.join(" ");
        assert!(
            all_args.contains("CAS Factory Supervisor"),
            "Codex supervisor should have supervisor instructions"
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_teams() {
        let teams = TeamsSpawnConfig {
            team_name: "test-team".to_string(),
            agent_id: "worker-1@test-team".to_string(),
            agent_name: "worker-1".to_string(),
            agent_color: "blue".to_string(),
            agent_type: "general-purpose".to_string(),
            parent_session_id: Some("lead-session-123".to_string()),
            lead_session_id: None,
            settings_path: None,
        };
        let config = PtyConfig::claude(
            "worker-1",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // effort
            Some(&teams),
        );
        assert!(config.args.contains(&"--team-name".to_string()));
        assert!(config.args.contains(&"test-team".to_string()));
        assert!(config.args.contains(&"--agent-id".to_string()));
        assert!(config.args.contains(&"worker-1@test-team".to_string()));
        assert!(config.args.contains(&"--agent-name".to_string()));
        assert!(config.args.contains(&"--agent-color".to_string()));
        assert!(config.args.contains(&"--teammate-mode".to_string()));
        assert!(config.args.contains(&"tmux".to_string()));
        assert!(config.args.contains(&"--parent-session-id".to_string()));
        assert!(config.args.contains(&"lead-session-123".to_string()));
        // Workers get --session-id for CAS agent auto-registration
        assert!(config.args.contains(&"--session-id".to_string()));
        assert!(
            config
                .env
                .iter()
                .any(|(k, v)| k == "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS" && v == "1")
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_custom_effort() {
        let config = PtyConfig::claude(
            "test-agent",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            Some("low"),  // effort override
            None,  // teams
        );
        let effort_idx = config
            .args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort must be present");
        assert_eq!(
            config.args[effort_idx + 1], "low",
            "custom effort should override hardcoded default"
        );
    }

    #[tokio::test]
    async fn test_pty_config_claude_supervisor_default_effort() {
        // When effort is None and role is supervisor, must default to "xhigh"
        let config = PtyConfig::claude(
            "sup",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // no effort — should fall back to "xhigh"
            None,  // teams
        );
        let effort_idx = config
            .args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort must be present");
        assert_eq!(config.args[effort_idx + 1], "xhigh");
    }

    #[tokio::test]
    async fn test_pty_config_claude_worker_default_effort() {
        // When effort is None and role is worker, must default to "high"
        let config = PtyConfig::claude(
            "wrk",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // no effort — should fall back to "high"
            None,  // teams
        );
        let effort_idx = config
            .args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort must be present");
        assert_eq!(config.args[effort_idx + 1], "high");
    }

    #[tokio::test]
    async fn test_pty_config_claude_with_teams_lead() {
        let teams = TeamsSpawnConfig {
            team_name: "test-team".to_string(),
            agent_id: "supervisor@test-team".to_string(),
            agent_name: "supervisor".to_string(),
            agent_color: "green".to_string(),
            agent_type: "team-lead".to_string(),
            parent_session_id: None,
            lead_session_id: None,
            settings_path: None,
        };
        let config = PtyConfig::claude(
            "supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,  // model
            None,  // effort
            Some(&teams),
        );
        // Lead also gets --teammate-mode so it polls its inbox
        assert!(config.args.contains(&"--teammate-mode".to_string()));
        assert!(config.args.contains(&"tmux".to_string()));
        // No --parent-session-id for the lead
        assert!(!config.args.contains(&"--parent-session-id".to_string()));
    }

    /// When `TeamsSpawnConfig::settings_path` is set (as it is for the
    /// supervisor in factory mode), the spawned `claude` invocation must
    /// include `--settings <path>` so Claude Code loads the allowlist that
    /// sidesteps the self-leadership routing deadlock. Workers without a
    /// `settings_path` must not get the flag.
    #[tokio::test]
    async fn test_pty_config_claude_teams_supervisor_gets_settings_flag() {
        let settings_path = "/home/pippenz/.claude/teams/deadlock-team/supervisor-settings.json";
        let teams = TeamsSpawnConfig {
            team_name: "deadlock-team".to_string(),
            agent_id: "supervisor@deadlock-team".to_string(),
            agent_name: "supervisor".to_string(),
            agent_color: "green".to_string(),
            agent_type: "team-lead".to_string(),
            parent_session_id: None,
            lead_session_id: None,
            settings_path: Some(settings_path.to_string()),
        };
        let config = PtyConfig::claude(
            "supervisor",
            "supervisor",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,  // effort
            Some(&teams),
        );
        assert!(
            config.args.contains(&"--settings".to_string()),
            "supervisor spawn must include --settings flag"
        );
        assert!(
            config.args.contains(&settings_path.to_string()),
            "supervisor spawn must pass the settings file path"
        );
    }

    /// Workers now ship their own settings file (cas-e15d). When
    /// `settings_path` is populated, the `--settings <path>` flag must
    /// appear in argv so `claude` loads the per-worker allowlist and the
    /// phantom `team-lead` escalation cannot fire.
    #[tokio::test]
    async fn test_pty_config_claude_teams_worker_gets_settings_flag() {
        let settings_path = "/home/pippenz/.claude/teams/deadlock-team/worker-1-settings.json";
        let teams = TeamsSpawnConfig {
            team_name: "deadlock-team".to_string(),
            agent_id: "worker-1@deadlock-team".to_string(),
            agent_name: "worker-1".to_string(),
            agent_color: "blue".to_string(),
            agent_type: "general-purpose".to_string(),
            parent_session_id: Some("lead-session-xyz".to_string()),
            lead_session_id: None,
            settings_path: Some(settings_path.to_string()),
        };
        let config = PtyConfig::claude(
            "worker-1",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,  // effort
            Some(&teams),
        );
        assert!(
            config.args.contains(&"--settings".to_string()),
            "worker spawn must include --settings flag"
        );
        assert!(
            config.args.contains(&settings_path.to_string()),
            "worker spawn must pass the worker settings file path"
        );
    }

    /// Argv builder contract: when `settings_path` is deliberately left as
    /// `None` (CLI usage, tests that opt out), the flag must be absent. This
    /// is the correctness gate for the `if let Some(..)` branch — not a
    /// statement about worker doctrine (workers get a path in production).
    #[tokio::test]
    async fn test_pty_config_claude_teams_no_settings_path_omits_flag() {
        let teams = TeamsSpawnConfig {
            team_name: "no-settings-team".to_string(),
            agent_id: "worker-bare@no-settings-team".to_string(),
            agent_name: "worker-bare".to_string(),
            agent_color: "blue".to_string(),
            agent_type: "general-purpose".to_string(),
            parent_session_id: Some("lead-session-xyz".to_string()),
            lead_session_id: None,
            settings_path: None,
        };
        let config = PtyConfig::claude(
            "worker-bare",
            "worker",
            PathBuf::from("/tmp"),
            None,
            None,
            None,
            None,
            None,  // effort
            Some(&teams),
        );
        assert!(
            !config.args.contains(&"--settings".to_string()),
            "no settings_path → argv must omit --settings"
        );
    }

    // cas-0bf4: resource-contention mitigation tests.
    //
    // These exercise `cargo_build_jobs_for_worker` and
    // `maybe_wrap_with_nice` plus their integration with
    // `PtyConfig::claude`. They poke process-wide env vars, so they
    // share a serializing mutex to avoid cross-test flakes when the
    // suite runs with multiple threads. Scope is per-test: each test
    // clears its own env on entry and on the exit via the guard.
    mod cas_0bf4_resource_contention {
        use crate::pty::*;
        use crate::pty::tests::ScopedEnv;

        #[test]
        fn cargo_build_jobs_honours_explicit_env_override() {
            let _e = ScopedEnv::new();
            // SAFETY: _e holds ENV_LOCK.
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "3");
            }
            assert_eq!(cargo_build_jobs_for_worker().as_deref(), Some("3"));
        }

        #[test]
        fn cargo_build_jobs_trims_whitespace_override() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "  6  ");
            }
            assert_eq!(cargo_build_jobs_for_worker().as_deref(), Some("6"));
        }

        #[test]
        fn cargo_build_jobs_auto_falls_through_to_computed() {
            let _e = ScopedEnv::new();
            // Explicit "auto" reads as fallthrough, computed value comes back.
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "auto");
            }
            let got = cargo_build_jobs_for_worker()
                .expect("available_parallelism should succeed on test host");
            let n: usize = got.parse().expect("computed CARGO_BUILD_JOBS must parse");
            assert!(n >= 2, "floor of 2 must hold even on 1–4 core hosts: got {n}");
        }

        #[test]
        fn cargo_build_jobs_empty_env_falls_through_to_computed() {
            let _e = ScopedEnv::new();
            // No env set at all → compute. Same assertion as "auto".
            let got = cargo_build_jobs_for_worker()
                .expect("available_parallelism should succeed on test host");
            let n: usize = got.parse().expect("computed CARGO_BUILD_JOBS must parse");
            assert!(n >= 2);
        }

        #[test]
        fn maybe_wrap_with_nice_is_noop_for_supervisor_role() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let (cmd, args) = maybe_wrap_with_nice(
                "claude",
                vec!["--session-id".to_string(), "abc".to_string()],
                "supervisor",
            );
            assert_eq!(cmd, "claude");
            assert_eq!(args, vec!["--session-id".to_string(), "abc".to_string()]);
        }

        #[test]
        fn maybe_wrap_with_nice_is_noop_without_env_sentinel() {
            let _e = ScopedEnv::new();
            // No CAS_FACTORY_NICE_WORKER set — passthrough for workers too.
            let (cmd, args) = maybe_wrap_with_nice(
                "claude",
                vec!["--foo".to_string()],
                "worker",
            );
            assert_eq!(cmd, "claude");
            assert_eq!(args, vec!["--foo".to_string()]);
        }

        #[test]
        fn maybe_wrap_with_nice_wraps_worker_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let (cmd, args) = maybe_wrap_with_nice(
                "claude",
                vec!["--session-id".to_string(), "xyz".to_string()],
                "worker",
            );
            assert_eq!(cmd, "nice");
            // Default level 10, original argv preserved after the wrapped command.
            assert_eq!(
                args,
                vec![
                    "-n".to_string(),
                    "10".to_string(),
                    "claude".to_string(),
                    "--session-id".to_string(),
                    "xyz".to_string(),
                ]
            );
        }

        #[test]
        fn maybe_wrap_with_nice_honours_level_override() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
                std::env::set_var("CAS_FACTORY_NICE_LEVEL", "15");
            }
            let (cmd, args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(cmd, "nice");
            assert_eq!(args[..2], ["-n".to_string(), "15".to_string()]);
            assert_eq!(args[2], "claude");
        }

        #[test]
        fn maybe_wrap_with_nice_rejects_non_1_sentinel_value() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "true"); // not "1"
            }
            let (cmd, _args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(cmd, "claude", "only the exact value '1' activates nice-wrap");
        }

        #[test]
        fn claude_worker_gets_cargo_build_jobs_env() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::claude(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,  // effort
                None,
            );
            assert!(
                config
                    .env
                    .iter()
                    .any(|(k, v)| k == "CARGO_BUILD_JOBS" && v == "4"),
                "worker PtyConfig must export CARGO_BUILD_JOBS when override is set"
            );
        }

        #[test]
        fn claude_supervisor_does_not_get_cargo_build_jobs_env() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::claude(
                "s1",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,  // effort
                None,
            );
            assert!(
                !config.env.iter().any(|(k, _)| k == "CARGO_BUILD_JOBS"),
                "supervisor must NOT get CARGO_BUILD_JOBS cap — only workers do"
            );
        }

        #[test]
        fn claude_worker_command_wraps_in_nice_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let config = PtyConfig::claude(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,  // effort
                None,
            );
            assert_eq!(config.command, "nice");
            assert_eq!(config.args[0], "-n");
            assert_eq!(config.args[2], "claude");
        }

        #[test]
        fn cargo_build_jobs_case_insensitive_auto_falls_through() {
            // cas-0bf4 adversarial P2: a user who writes "Auto" or "AUTO"
            // in config must not leak the literal string into
            // CARGO_BUILD_JOBS (cargo would reject it as a non-integer
            // and silently defeat the cap).
            let _e = ScopedEnv::new();
            for variant in ["Auto", "AUTO", "auto", "  Auto  "] {
                unsafe {
                    std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", variant);
                }
                let got = cargo_build_jobs_for_worker()
                    .expect("available_parallelism should succeed on test host");
                let n: usize = got.parse().expect("computed value must parse as integer");
                assert!(n >= 2, "variant {variant:?} should fall through to auto-compute, got {got}");
            }
        }

        #[test]
        fn maybe_wrap_with_nice_rejects_non_numeric_level() {
            // cas-0bf4 correctness P2: a non-numeric NICE_LEVEL must not
            // reach `nice -n <garbage>` — would fail every worker spawn.
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
                std::env::set_var("CAS_FACTORY_NICE_LEVEL", "high");
            }
            let (cmd, args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(cmd, "nice");
            assert_eq!(args[..2], ["-n".to_string(), "10".to_string()],
                "non-numeric NICE_LEVEL must fall back to default 10");
        }

        #[test]
        fn maybe_wrap_with_nice_accepts_negative_numeric_level() {
            // Negative values parse as valid i32 and pass through; `nice`
            // itself rejects them for non-root, which is a separate OS
            // concern outside this helper. Documents the contract so a
            // future clamp-to-positive refactor is an explicit decision.
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
                std::env::set_var("CAS_FACTORY_NICE_LEVEL", "-5");
            }
            let (_cmd, args) = maybe_wrap_with_nice("claude", vec![], "worker");
            assert_eq!(args[1], "-5");
        }

        #[test]
        fn codex_worker_gets_cargo_build_jobs_env() {
            // cas-0bf4 testing P1: codex spawn path must mirror claude.
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::codex(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,  // effort
                None,
            );
            assert!(
                config
                    .env
                    .iter()
                    .any(|(k, v)| k == "CARGO_BUILD_JOBS" && v == "4"),
                "codex worker PtyConfig must export CARGO_BUILD_JOBS when override is set"
            );
        }

        #[test]
        fn codex_supervisor_does_not_get_cargo_build_jobs_env() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", "4");
            }
            let config = PtyConfig::codex(
                "s1",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,  // effort
                None,
            );
            assert!(
                !config.env.iter().any(|(k, _)| k == "CARGO_BUILD_JOBS"),
                "codex supervisor must NOT get CARGO_BUILD_JOBS cap"
            );
        }

        #[test]
        fn codex_worker_command_wraps_in_nice_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let config = PtyConfig::codex(
                "w1",
                "worker",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,  // effort
                None,
            );
            assert_eq!(config.command, "nice");
            assert_eq!(config.args[0], "-n");
            assert_eq!(config.args[2], "codex");
        }

        #[test]
        fn claude_supervisor_command_unwrapped_even_when_sentinel_set() {
            let _e = ScopedEnv::new();
            unsafe {
                std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
            }
            let config = PtyConfig::claude(
                "s1",
                "supervisor",
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,  // effort
                None,
            );
            assert_eq!(
                config.command, "claude",
                "supervisor must not be niced — the whole point is it stays at nice 0"
            );
        }
    }
}
