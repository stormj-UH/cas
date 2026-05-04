//! Main multiplexer
//!
//! Manages multiple panes, handles input routing, and coordinates rendering.

use crate::error::{Error, Result};
use crate::harness::SupervisorCli;
use crate::pane::{Pane, PaneId, PaneKind};
use crate::pty::{PtyConfig, PtyEvent, TeamsSpawnConfig};
use cas_factory_protocol::ServerMessage;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Configuration for the multiplexer
#[derive(Debug, Clone)]
pub struct MuxConfig {
    /// Working directory for agents (supervisor and fallback for workers)
    pub cwd: PathBuf,
    /// Path to the .cas directory (if set, passes CAS_ROOT env var to agents)
    /// This allows workers in clone directories to access the main repo's CAS state.
    pub cas_root: Option<PathBuf>,
    /// Per-worker working directories (worker_name -> clone path)
    /// Workers not in this map use the default `cwd`.
    pub worker_cwds: HashMap<String, PathBuf>,
    /// Number of worker agents
    pub workers: usize,
    /// Worker names (if not provided, generated)
    pub worker_names: Vec<String>,
    /// Supervisor name
    pub supervisor_name: String,
    /// Supervisor CLI (codex, claude, or pi)
    pub supervisor_cli: SupervisorCli,
    /// Worker CLI (codex, claude, or pi)
    pub worker_cli: SupervisorCli,
    /// Model for supervisor (passed as --model flag)
    pub supervisor_model: Option<String>,
    /// Model for workers (passed as --model flag)
    pub worker_model: Option<String>,
    /// Reasoning effort for supervisor (passed as --effort flag; defaults to "high")
    pub supervisor_effort: Option<String>,
    /// Reasoning effort for workers (passed as --effort flag; defaults to "high")
    pub worker_effort: Option<String>,
    /// Include director pane
    pub include_director: bool,
    /// Terminal size
    pub rows: u16,
    pub cols: u16,
    /// Per-agent Teams spawn configs (agent_name -> config).
    /// When set, agents are spawned with native Agent Teams CLI flags.
    pub teams_configs: HashMap<String, TeamsSpawnConfig>,
}

impl Default for MuxConfig {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            cas_root: None,
            worker_cwds: HashMap::new(),
            workers: 2,
            worker_names: vec![],
            supervisor_name: "supervisor".to_string(),
            supervisor_cli: SupervisorCli::Codex,
            worker_cli: SupervisorCli::Claude,
            supervisor_model: None,
            worker_model: None,
            supervisor_effort: None,
            worker_effort: None,
            include_director: true,
            rows: 24,
            cols: 80,
            teams_configs: HashMap::new(),
        }
    }
}

/// Events from the multiplexer
#[derive(Debug, Clone)]
pub enum MuxEvent {
    /// A pane received output (includes raw bytes for client-side rendering)
    PaneOutput {
        pane_id: PaneId,
        /// Raw PTY bytes for client-side terminal emulation
        data: Vec<u8>,
    },
    /// A pane's process exited
    PaneExited {
        pane_id: PaneId,
        exit_code: Option<i32>,
    },
    /// Focus changed
    FocusChanged { from: Option<PaneId>, to: PaneId },
    /// A pane was added
    PaneAdded { pane_id: PaneId },
    /// A pane was removed
    PaneRemoved { pane_id: PaneId },
}

/// The main terminal multiplexer
pub struct Mux {
    /// All panes, keyed by ID (insertion order preserved)
    panes: IndexMap<PaneId, Pane>,
    /// Currently focused pane
    focused: Option<PaneId>,
    /// Event sender
    event_tx: mpsc::Sender<MuxEvent>,
    /// Event receiver
    event_rx: mpsc::Receiver<MuxEvent>,
    /// Terminal size
    rows: u16,
    cols: u16,
    /// Worker CLI used for dynamic worker spawns
    worker_cli: SupervisorCli,
    /// Worker model used for dynamic worker spawns
    worker_model: Option<String>,
    /// Worker reasoning effort used for dynamic worker spawns
    worker_effort: Option<String>,
}

impl Mux {
    /// Create a new multiplexer
    pub fn new(rows: u16, cols: u16) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        Self {
            panes: IndexMap::new(),
            focused: None,
            event_tx,
            event_rx,
            rows,
            cols,
            worker_cli: SupervisorCli::Claude,
            worker_model: None,
            worker_effort: None,
        }
    }

    /// Set CLI used for worker pane spawns.
    pub fn set_worker_cli(&mut self, worker_cli: SupervisorCli) {
        self.worker_cli = worker_cli;
    }

    /// Set model used for worker pane spawns.
    pub fn set_worker_model(&mut self, model: Option<String>) {
        self.worker_model = model;
    }

    /// Set reasoning effort used for worker pane spawns.
    pub fn set_worker_effort(&mut self, effort: Option<String>) {
        self.worker_effort = effort;
    }

    /// Build the `PtyConfig`s that `factory()` would spawn for each worker and
    /// supervisor pane, without actually spawning any processes.
    ///
    /// Returns `(agent_name, PtyConfig)` pairs — workers first, then the
    /// supervisor. The director pane (no PTY) is excluded.
    ///
    /// Primary use: integration tests that need to verify effort / model args
    /// flow from `MuxConfig` all the way to the CLI subprocess arguments,
    /// without requiring a real `claude` or `codex` binary.
    pub fn factory_pane_configs(config: &MuxConfig) -> Vec<(String, PtyConfig)> {
        let worker_names: Vec<String> = if config.worker_names.is_empty() {
            (0..config.workers)
                .map(|i| format!("worker-{}", i + 1))
                .collect()
        } else {
            config.worker_names.clone()
        };

        let mut result = Vec::with_capacity(worker_names.len() + 1);

        for name in &worker_names {
            let worker_cwd = config
                .worker_cwds
                .get(name)
                .cloned()
                .unwrap_or_else(|| config.cwd.clone());
            let teams = config.teams_configs.get(name);
            let pty_config = Pane::build_worker_config(
                name,
                worker_cwd,
                config.cas_root.as_ref(),
                &config.supervisor_name,
                config.worker_cli,
                config.worker_model.as_deref(),
                config.worker_effort.as_deref(),
                teams,
            );
            result.push((name.clone(), pty_config));
        }

        let sup_teams = config.teams_configs.get(&config.supervisor_name);
        let sup_config = Pane::build_supervisor_config(
            &config.supervisor_name,
            config.cwd.clone(),
            config.cas_root.as_ref(),
            config.supervisor_cli,
            config.worker_cli,
            &worker_names,
            config.supervisor_model.as_deref(),
            config.supervisor_effort.as_deref(),
            sup_teams,
        );
        result.push((config.supervisor_name.clone(), sup_config));

        result
    }

    /// Create a multiplexer with factory configuration
    pub fn factory(config: MuxConfig) -> Result<Self> {
        let mut mux = Self::new(config.rows, config.cols);
        mux.set_worker_cli(config.worker_cli);
        mux.set_worker_model(config.worker_model.clone());
        mux.set_worker_effort(config.worker_effort.clone());

        // Calculate pane sizes based on layout
        // Layout: [Workers] [Supervisor] [Director]
        let num_panes = config.workers + 1 + if config.include_director { 1 } else { 0 };
        let pane_cols = config.cols / num_panes as u16;
        let pane_rows = config.rows;

        // Create worker panes
        let worker_names: Vec<String> = if config.worker_names.is_empty() {
            (0..config.workers)
                .map(|i| format!("worker-{}", i + 1))
                .collect()
        } else {
            config.worker_names.clone()
        };

        for name in &worker_names {
            // Use worker-specific CWD if available, otherwise fall back to default
            let worker_cwd = config
                .worker_cwds
                .get(name)
                .cloned()
                .unwrap_or_else(|| config.cwd.clone());
            let teams = config.teams_configs.get(name);
            let pane = Pane::worker(
                name,
                worker_cwd,
                config.cas_root.as_ref(),
                &config.supervisor_name,
                config.worker_cli,
                config.worker_model.as_deref(),
                config.worker_effort.as_deref(),
                pane_rows,
                pane_cols,
                teams,
            )?;
            mux.add_pane(pane);
        }

        // Create supervisor pane (always uses main cwd)
        let sup_teams = config.teams_configs.get(&config.supervisor_name);
        let supervisor = Pane::supervisor(
            &config.supervisor_name,
            config.cwd.clone(),
            config.cas_root.as_ref(),
            pane_rows,
            pane_cols,
            config.supervisor_cli,
            config.worker_cli,
            &worker_names,
            config.supervisor_model.as_deref(),
            config.supervisor_effort.as_deref(),
            sup_teams,
        )?;
        mux.add_pane(supervisor);

        // Create director pane (no PTY)
        if config.include_director {
            let director = Pane::director("director", pane_rows, pane_cols)?;
            mux.add_pane(director);
        }

        // Focus the first worker
        if let Some(first) = worker_names.first() {
            mux.focus(first);
        }

        Ok(mux)
    }

    /// Add a pane to the multiplexer
    pub fn add_pane(&mut self, pane: Pane) {
        let id = pane.id().to_string();
        self.panes.insert(id.clone(), pane);

        // If no pane is focused, focus this one
        if self.focused.is_none() {
            self.focused = Some(id.clone());
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.set_focused(true);
            }
        }

        let _ = self.event_tx.try_send(MuxEvent::PaneAdded { pane_id: id });
    }

    /// Remove a pane
    pub fn remove_pane(&mut self, id: &str) -> Option<Pane> {
        let pane = self.panes.shift_remove(id);

        // If we removed the focused pane, focus the next one
        if self.focused.as_deref() == Some(id) {
            self.focused = self.panes.keys().next().cloned();
            if let Some(new_focus) = &self.focused
                && let Some(pane) = self.panes.get_mut(new_focus)
            {
                pane.set_focused(true);
            }
        }

        if pane.is_some() {
            let _ = self.event_tx.try_send(MuxEvent::PaneRemoved {
                pane_id: id.to_string(),
            });
        }

        pane
    }

    /// Get a pane by ID
    pub fn get(&self, id: &str) -> Option<&Pane> {
        self.panes.get(id)
    }

    /// Get a mutable pane by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Pane> {
        self.panes.get_mut(id)
    }

    /// Get the focused pane
    pub fn focused(&self) -> Option<&Pane> {
        self.focused.as_ref().and_then(|id| self.panes.get(id))
    }

    /// Get the focused pane mutably
    pub fn focused_mut(&mut self) -> Option<&mut Pane> {
        if let Some(id) = self.focused.clone() {
            self.panes.get_mut(&id)
        } else {
            None
        }
    }

    /// Get the focused pane ID
    pub fn focused_id(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// Focus a pane by ID
    pub fn focus(&mut self, id: &str) -> bool {
        if !self.panes.contains_key(id) {
            return false;
        }

        let old_focus = self.focused.take();

        // Unfocus old pane
        if let Some(old_id) = &old_focus
            && let Some(pane) = self.panes.get_mut(old_id)
        {
            pane.set_focused(false);
        }

        // Focus new pane
        self.focused = Some(id.to_string());
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_focused(true);
        }

        let _ = self.event_tx.try_send(MuxEvent::FocusChanged {
            from: old_focus,
            to: id.to_string(),
        });

        true
    }

    /// Focus the next pane
    pub fn focus_next(&mut self) {
        let len = self.panes.len();
        if len == 0 {
            return;
        }

        let current_idx = self
            .focused
            .as_ref()
            .and_then(|f| self.panes.get_index_of(f))
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % len;
        if let Some((id, _)) = self.panes.get_index(next_idx) {
            let id = id.clone();
            self.focus(&id);
        }
    }

    /// Focus the previous pane
    pub fn focus_prev(&mut self) {
        let len = self.panes.len();
        if len == 0 {
            return;
        }

        let current_idx = self
            .focused
            .as_ref()
            .and_then(|f| self.panes.get_index_of(f))
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 {
            len - 1
        } else {
            current_idx - 1
        };
        if let Some((id, _)) = self.panes.get_index(prev_idx) {
            let id = id.clone();
            self.focus(&id);
        }
    }

    /// Get all pane IDs
    pub fn pane_ids(&self) -> Vec<&str> {
        self.panes.keys().map(|s| s.as_str()).collect()
    }

    /// Get all panes
    pub fn panes(&self) -> impl Iterator<Item = &Pane> {
        self.panes.values()
    }

    /// Get all panes mutably
    pub fn panes_mut(&mut self) -> impl Iterator<Item = &mut Pane> {
        self.panes.values_mut()
    }

    /// Get panes of a specific kind
    pub fn panes_by_kind(&self, kind: PaneKind) -> impl Iterator<Item = &Pane> {
        self.panes.values().filter(move |p| *p.kind() == kind)
    }

    /// Get worker panes
    pub fn workers(&self) -> impl Iterator<Item = &Pane> {
        self.panes_by_kind(PaneKind::Worker)
    }

    /// Get the count of all panes
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Get the count of worker panes
    pub fn worker_count(&self) -> usize {
        self.panes
            .values()
            .filter(|p| *p.kind() == PaneKind::Worker)
            .count()
    }

    /// Add a worker pane at runtime
    ///
    /// This is the primary method for dynamic worker spawning.
    ///
    /// # Arguments
    /// * `name` - Worker name (also used as pane ID)
    /// * `cwd` - Working directory for the worker (typically a clone directory)
    /// * `cas_root` - Optional path to .cas directory for CAS_ROOT env var
    /// * `supervisor_name` - Name of the supervisor (enables `target: supervisor` in message action)
    ///
    /// # Returns
    /// The pane ID on success
    pub fn add_worker(
        &mut self,
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: &str,
        teams: Option<&TeamsSpawnConfig>,
    ) -> Result<PaneId> {
        // Check if pane with this name already exists
        if self.panes.contains_key(name) {
            return Err(Error::pty(format!("Pane '{name}' already exists")));
        }

        let pane = Pane::worker(
            name,
            cwd,
            cas_root,
            supervisor_name,
            self.worker_cli,
            self.worker_model.as_deref(),
            self.worker_effort.as_deref(),
            self.rows,
            self.cols,
            teams,
        )?;
        let id = pane.id().to_string();
        self.add_pane(pane);
        Ok(id)
    }

    /// Add a new shell pane to the mux.
    pub fn add_shell(
        &mut self,
        name: &str,
        cwd: PathBuf,
        shell_command: Option<&str>,
    ) -> Result<PaneId> {
        if self.panes.contains_key(name) {
            return Err(Error::pty(format!("Pane '{name}' already exists")));
        }
        let pane = Pane::shell(name, cwd, shell_command, self.rows, self.cols)?;
        let id = pane.id().to_string();
        self.add_pane(pane);
        Ok(id)
    }

    /// Remove a shell pane by name.
    pub fn remove_shell(&mut self, name: &str) -> Result<()> {
        if let Some(pane) = self.panes.get(name) {
            if *pane.kind() != PaneKind::Shell {
                return Err(Error::pty(format!(
                    "Pane '{}' is not a shell (is {:?})",
                    name,
                    pane.kind()
                )));
            }
        } else {
            return Err(Error::pane_not_found(name));
        }
        self.remove_pane(name);
        Ok(())
    }

    /// Remove a worker pane by name and cleanup its PTY
    ///
    /// This is the primary method for dynamic worker shutdown.
    /// The pane's PTY will be dropped, sending SIGHUP to the process.
    pub fn remove_worker(&mut self, name: &str) -> Result<()> {
        // Verify it's a worker
        if let Some(pane) = self.panes.get(name) {
            if *pane.kind() != PaneKind::Worker {
                return Err(Error::pty(format!(
                    "Pane '{}' is not a worker (is {:?})",
                    name,
                    pane.kind()
                )));
            }
        } else {
            return Err(Error::pane_not_found(name));
        }

        // Remove the pane (this drops the PTY, sending SIGHUP)
        self.remove_pane(name);
        Ok(())
    }

    /// Get the supervisor pane
    pub fn supervisor(&self) -> Option<&Pane> {
        self.panes_by_kind(PaneKind::Supervisor).next()
    }

    /// Get the director pane
    pub fn director(&self) -> Option<&Pane> {
        self.panes_by_kind(PaneKind::Director).next()
    }

    /// Check whether a pane is ready to accept prompt injection.
    pub fn pane_ready_for_injection(&self, pane_id: &str) -> bool {
        self.panes
            .get(pane_id)
            .map(|p| p.ready_for_injection())
            .unwrap_or(false)
    }

    /// Inject a prompt into a specific pane
    pub async fn inject(&self, pane_id: &str, prompt: &str) -> Result<()> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.inject_prompt(prompt).await
    }

    /// Inject a prompt into the focused pane
    pub async fn inject_focused(&self, prompt: &str) -> Result<()> {
        let pane = self
            .focused()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.inject_prompt(prompt).await
    }

    /// Inject a prompt into all workers
    pub async fn inject_all_workers(&self, prompt: &str) -> Result<()> {
        for pane in self.workers() {
            pane.inject_prompt(prompt).await?;
        }
        Ok(())
    }

    /// Inject a prompt into the supervisor
    pub async fn inject_supervisor(&self, prompt: &str) -> Result<()> {
        let pane = self
            .supervisor()
            .ok_or_else(|| Error::pane_not_found("supervisor"))?;
        pane.inject_prompt(prompt).await
    }

    /// Send input to the focused pane
    pub async fn send_input(&self, data: &[u8]) -> Result<()> {
        let pane = self
            .focused()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.write(data).await
    }

    /// Send input to a specific pane.
    pub async fn send_input_to(&self, pane_id: &str, data: &[u8]) -> Result<()> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.write(data).await
    }

    /// Poll all panes for events (non-blocking)
    ///
    /// Returns one event at a time. Call in a loop until None to process all events.
    pub fn poll(&mut self) -> Option<MuxEvent> {
        // First check the event queue
        if let Ok(event) = self.event_rx.try_recv() {
            return Some(event);
        }

        // Poll each pane for ONE event (not draining all)
        // This ensures we return events as they come without losing any
        for (id, pane) in self.panes.iter_mut() {
            if let Some(event) = pane.poll() {
                match event {
                    PtyEvent::Output(data) => {
                        return Some(MuxEvent::PaneOutput {
                            pane_id: id.clone(),
                            data,
                        });
                    }
                    PtyEvent::Exited(code) => {
                        return Some(MuxEvent::PaneExited {
                            pane_id: id.clone(),
                            exit_code: code,
                        });
                    }
                    PtyEvent::Error(e) => {
                        tracing::error!("PTY error in pane {}: {}", id, e);
                        // Continue to next pane/event
                    }
                }
            }
        }

        None
    }

    /// Poll all panes and drain all available events at once (more efficient for multi-pane)
    ///
    /// Returns (total_bytes, events). Uses coalesced output feeding for efficiency when
    /// multiple Claude instances are generating long responses simultaneously.
    ///
    /// The MuxEvent::PaneOutput events include the raw PTY bytes so WebSocket clients
    /// can feed them to their own terminal emulators.
    pub fn poll_batch(&mut self) -> (usize, Vec<MuxEvent>) {
        let mut events = Vec::new();
        let mut total_bytes = 0;

        // First drain the event queue
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }

        // Drain each pane using coalesced output (more efficient for high throughput)
        for (id, pane) in self.panes.iter_mut() {
            let (data, other_events) = pane.drain_output();

            if !data.is_empty() {
                total_bytes += data.len();
                // Include raw data for WebSocket clients
                events.push(MuxEvent::PaneOutput {
                    pane_id: id.clone(),
                    data,
                });
            }

            // Forward non-output events
            for event in other_events {
                match event {
                    PtyEvent::Exited(code) => {
                        events.push(MuxEvent::PaneExited {
                            pane_id: id.clone(),
                            exit_code: code,
                        });
                    }
                    PtyEvent::Error(e) => {
                        tracing::error!("PTY error in pane {}: {}", id, e);
                    }
                    _ => {}
                }
            }
        }

        (total_bytes, events)
    }

    /// Get incremental terminal updates for all panes with changes.
    ///
    /// Returns a list of (pane_id, ServerMessage::PaneRowsUpdate) for each pane
    /// that has dirty rows since the last call. This is the tmux-style approach
    /// where the server renders terminals and sends pre-rendered cells to clients.
    ///
    /// Call this after `poll_batch()` to get rendered updates instead of raw bytes.
    pub fn get_incremental_updates(&mut self) -> Vec<ServerMessage> {
        let mut updates = Vec::new();

        for (id, pane) in self.panes.iter_mut() {
            match pane.get_incremental_update() {
                Ok(Some((rows, cursor, seq))) => {
                    updates.push(ServerMessage::PaneRowsUpdate {
                        pane_id: id.clone(),
                        rows,
                        cursor,
                        seq,
                    });
                }
                Ok(None) => {
                    // No updates for this pane
                }
                Err(e) => {
                    tracing::warn!("Pane {}: get_incremental_update failed: {}", id, e);
                }
            }
        }

        updates
    }

    /// Get full terminal snapshot for a specific pane.
    ///
    /// Used for initial sync when a client connects or when scrollback is requested.
    pub fn get_pane_snapshot(
        &self,
        pane_id: &str,
    ) -> Option<(cas_factory_protocol::TerminalSnapshot, u32, u32)> {
        self.panes.get(pane_id).and_then(|pane| {
            pane.get_full_snapshot()
                .ok()
                .map(|snapshot| (snapshot, pane.scroll_offset(), pane.scrollback_lines()))
        })
    }

    /// Receive the next event (blocking)
    pub async fn recv(&mut self) -> Option<MuxEvent> {
        self.event_rx.recv().await
    }

    /// Resize all panes
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.rows = rows;
        self.cols = cols;

        // Recalculate pane sizes
        let num_panes = self.panes.len();
        if num_panes == 0 {
            return Ok(());
        }

        let pane_cols = cols / num_panes as u16;
        let pane_rows = rows;

        for pane in self.panes.values_mut() {
            pane.resize(pane_rows, pane_cols)?;
        }

        Ok(())
    }

    /// Get the terminal size
    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Interrupt the focused pane
    pub async fn interrupt_focused(&self) -> Result<()> {
        let pane = self
            .focused()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.interrupt().await
    }

    /// Interrupt a specific pane by ID
    pub async fn interrupt(&self, pane_id: &str) -> Result<()> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.interrupt().await
    }

    /// Scroll the focused pane by delta lines
    ///
    /// Positive delta scrolls down (towards newer content), negative scrolls up (towards older content).
    pub fn scroll_focused(&mut self, delta: i32) -> Result<()> {
        let pane = self
            .focused_mut()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.scroll(delta)
    }

    /// Scroll the focused pane to top of scrollback
    pub fn scroll_focused_to_top(&mut self) -> Result<()> {
        let pane = self
            .focused_mut()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.scroll_to_top()
    }

    /// Scroll the focused pane to bottom (most recent content)
    pub fn scroll_focused_to_bottom(&mut self) -> Result<()> {
        let pane = self
            .focused_mut()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.scroll_to_bottom()
    }

    /// Scroll a specific pane by delta lines
    pub fn scroll_pane(&mut self, pane_id: &str, delta: i32) -> Result<()> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.scroll(delta)
    }

    /// Scroll a specific pane and return snapshot with cache rows for smooth scrolling.
    ///
    /// This is the main entry point for handling Scroll messages with cache_window.
    /// Returns (snapshot, cache_rows, cache_start_row, scroll_offset, scrollback_lines).
    #[allow(clippy::type_complexity)]
    pub fn scroll_pane_with_cache(
        &mut self,
        pane_id: &str,
        delta: i32,
        cache_window: u32,
    ) -> Result<(
        cas_factory_protocol::TerminalSnapshot,
        Vec<cas_factory_protocol::CacheRow>,
        Option<u32>,
        u32,
        u32,
    )> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;

        // Apply scroll
        pane.scroll(delta)?;

        // Get snapshot with cache rows
        let (snapshot, cache_rows, cache_start) = pane.create_snapshot_with_cache(cache_window)?;
        let scroll_offset = pane.scroll_offset();
        let scrollback_lines = pane.scrollback_lines();

        Ok((
            snapshot,
            cache_rows,
            cache_start,
            scroll_offset,
            scrollback_lines,
        ))
    }

    /// Scroll a specific pane and return RowData snapshot with cache rows.
    ///
    /// Returns (snapshot_rows, cache_rows, cache_start_row, scroll_offset, scrollback_lines).
    #[allow(clippy::type_complexity)]
    pub fn scroll_pane_with_cache_rows(
        &mut self,
        pane_id: &str,
        delta: i32,
        cache_window: u32,
    ) -> Result<(
        Vec<cas_factory_protocol::RowData>,
        Vec<cas_factory_protocol::CacheRow>,
        Option<u32>,
        u32,
        u32,
    )> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;

        // Apply scroll
        pane.scroll(delta)?;

        // Get snapshot rows with cache
        let (snapshot_rows, cache_rows, cache_start) =
            pane.create_snapshot_rows_with_cache(cache_window)?;
        let scroll_offset = pane.scroll_offset();
        let scrollback_lines = pane.scrollback_lines();

        Ok((
            snapshot_rows,
            cache_rows,
            cache_start,
            scroll_offset,
            scrollback_lines,
        ))
    }

    /// Get the current scroll offset for a pane (lines from bottom).
    pub fn scroll_offset(&self, pane_id: &str) -> Option<u32> {
        self.get(pane_id).map(|p| p.scroll_offset())
    }

    /// Scroll a specific pane to bottom
    pub fn scroll_pane_to_bottom(&mut self, pane_id: &str) -> Result<()> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.scroll_to_bottom()
    }

    /// Kill all panes (terminate all PTY processes)
    ///
    /// This should be called during shutdown to ensure all child processes are terminated.
    pub fn kill_all(&mut self) {
        for pane in self.panes.values_mut() {
            pane.kill();
        }
    }
}

#[cfg(test)]
#[path = "mux_tests/tests.rs"]
mod tests;
