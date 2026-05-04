use crate::ui::factory::daemon::imports::*;
use crate::ui::factory::director::AgentSummary;

impl FactoryDaemon {
    pub(super) fn handle_mux_event(&mut self, event: cas_mux::MuxEvent) {
        match event {
            cas_mux::MuxEvent::PaneOutput { pane_id, data } => {
                // Always buffer raw PTY bytes (warm buffer for future viewers)
                self.buffer_pane_output(&pane_id, &data);
                // Forward to any active web viewers
                self.forward_pane_output(&pane_id, &data);
                // Forward to GUI and WebSocket clients
                self.forward_pane_output_to_gui(&pane_id, &data);
                self.forward_pane_output_to_ws(&pane_id, &data);
            }
            cas_mux::MuxEvent::PaneExited { pane_id, exit_code } => {
                // Notify GUI and WS clients
                self.gui_notify_pane_exited(&pane_id, exit_code);
                let is_supervisor = pane_id == self.app.supervisor_name();
                let is_worker = self.app.worker_names().contains(&pane_id);

                if is_supervisor {
                    // Supervisor exited (either /exit or crash) — shut down the whole factory
                    tracing::info!("Supervisor exited with code {exit_code:?}, shutting down");
                    self.shutdown.store(true, Ordering::Relaxed);
                } else if is_worker {
                    let _ = self.handle_worker_crash(&pane_id, exit_code);
                }
            }
            _ => {}
        }
    }

    /// Handle worker crash
    fn handle_worker_crash(
        &mut self,
        worker_name: &str,
        exit_code: Option<i32>,
    ) -> anyhow::Result<()> {
        let agent_store = open_agent_store(self.app.cas_dir())?;

        // Look up agent by name
        let agent_id = self
            .app
            .director_data()
            .agents
            .iter()
            .find(|a| is_exact_agent_name_match(a, worker_name))
            .map(|a| a.id.clone());

        if let Some(id) = agent_id {
            let _ = agent_store.mark_stale(&id);
        }

        self.app.mark_worker_crashed(worker_name);
        self.dead_workers.insert(worker_name.to_string());

        let exit_info = match exit_code {
            Some(0) => "exited normally".to_string(),
            Some(code) => format!("crashed with exit code {code}"),
            None => "was terminated".to_string(),
        };

        self.app
            .set_error(format!("Worker '{worker_name}' {exit_info}"));
        self.app.notifier().notify_crash(worker_name, &exit_info);

        Ok(())
    }

    /// Check if a message source is a dead (shutdown/crashed) worker.
    ///
    /// Returns true only for sources that were known factory workers but have
    /// since been removed. External sources (openclaw, bridge, etc.) pass through.
    fn is_dead_worker_source(&self, source: &str) -> bool {
        self.dead_workers.contains(source)
    }

    /// Detect idle-like messages that don't carry new information.
    ///
    /// The daemon rate-limits these (1 per 5 min per source) and silently
    /// marks the rest as processed, so any false positive here is a
    /// *dropped message*, not merely a noisy one.
    ///
    /// Matching rules (intentionally strict):
    ///   1. The message must be short (<= 300 chars). A long message with
    ///      "standing by" buried in it is almost certainly a real status
    ///      report, not an idle heartbeat.
    ///   2. The trimmed, lowercased message must **start with** one of the
    ///      stock idle phrases. Substring matches were dropping messages
    ///      like "Fix 1 for the WorkerIdle debounce race" or "the idle
    ///      detector emits …" — both of which contain the literal word
    ///      "idle" but are clearly not idle heartbeats.
    ///
    /// Previously this used unanchored substring matches including a bare
    /// `"idle"` and the phrase `"mcp tools unavailable"`, which produced
    /// false positives on legitimate status/debug messages. See cas-f9e8.
    fn is_idle_message(text: &str) -> bool {
        const MAX_IDLE_LEN: usize = 300;

        // Stock idle heartbeats workers are instructed to send when they
        // have nothing to do. Must be lowercase and pre-trimmed for the
        // `starts_with` check below to be meaningful.
        const IDLE_PREFIXES: &[&str] = &[
            "standing by",
            "ready for task",
            "ready for tasks",
            "awaiting instructions",
            "awaiting task",
            "awaiting tasks",
            "waiting for work",
            "no task assigned",
            "no tasks assigned",
        ];

        if text.len() > MAX_IDLE_LEN {
            return false;
        }
        let lower = text.trim().to_lowercase();
        IDLE_PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
    }

    /// Process prompt queue
    pub(super) async fn process_prompt_queue(&mut self) -> anyhow::Result<()> {
        use cas_store::{EventStore, SqliteEventStore};
        use cas_types::{Event, EventEntityType, EventType};

        let queue = open_prompt_queue_store(self.app.cas_dir())?;

        // Build target list: this session's supervisor + workers + "all_workers".
        // This prevents us from consuming messages meant for a different factory
        // session running in the same project directory.
        let supervisor_name = self.app.supervisor_name().to_string();
        let worker_names = self.app.worker_names().to_vec();
        let mut targets: Vec<&str> = Vec::with_capacity(worker_names.len() + 2);
        targets.push(&supervisor_name);
        targets.push("all_workers");
        for w in &worker_names {
            targets.push(w.as_str());
        }

        // Peek first, only ack after successful injection to provide at-least-once delivery.
        // Filter by targets AND session to prevent cross-session message theft.
        let prompts = queue.peek_for_targets(&targets, Some(&self.session_name), 10)?;

        if !prompts.is_empty() {
            tracing::info!("Processing {} prompts from queue", prompts.len());

            // cas-f9e8 telemetry: record the wait each message spent in the
            // queue before the daemon picked it up. The gap between `now`
            // and `queued.created_at` is the queue→deliver latency, which
            // is what the P99 SLO targets. Logged at debug; enable via
            // `RUST_LOG=cas::coordination=debug`.
            let now = chrono::Utc::now();
            for queued in &prompts {
                let wait_ms = (now - queued.created_at).num_milliseconds();
                tracing::debug!(
                    target: "cas::coordination",
                    stage = "daemon_pickup",
                    channel = "prompt_queue",
                    message_id = queued.id,
                    source = %queued.source,
                    target_agent = %queued.target,
                    priority = ?queued.priority,
                    wait_ms,
                    "prompt_queue message picked up by daemon"
                );
            }
        }

        // Best-effort event recording (for external tooling acks, activity feed, playback).
        let event_store = SqliteEventStore::open(self.app.cas_dir()).ok();

        // Build a set of agent names with native_extension=true.
        // These agents handle message delivery via their own extension
        // so we skip PTY text injection for them.
        let native_agents: std::collections::HashSet<String> = open_agent_store(self.app.cas_dir())
            .ok()
            .and_then(|store| store.list(None).ok())
            .map(|agents| {
                agents
                    .into_iter()
                    .filter(|a| {
                        a.metadata
                            .get("native_extension")
                            .map(|v| v == "true")
                            .unwrap_or(false)
                    })
                    .map(|a| a.name)
                    .collect()
            })
            .unwrap_or_default();

        for queued in prompts {
            let target = &queued.target;

            // Suppress messages from workers that have been shut down or crashed.
            // These workers are no longer in the session and their messages (especially
            // idle notifications) would just add noise to the supervisor context.
            if self.is_dead_worker_source(&queued.source) {
                tracing::debug!(
                    prompt_id = queued.id,
                    source = %queued.source,
                    target = %queued.target,
                    "Dropping message from dead worker"
                );
                let _ = queue.mark_processed(queued.id);
                continue;
            }

            // Dedup idle-like messages from the same worker (max 1 per 5 minutes).
            // Workers often send repeated "standing by", "ready", "idle" messages
            // that flood the supervisor context without adding information.
            if Self::is_idle_message(&queued.prompt) {
                let now = std::time::Instant::now();
                let dominated = self
                    .last_idle_message_times
                    .get(&queued.source)
                    .is_some_and(|last| now.duration_since(*last) < std::time::Duration::from_secs(300));
                if dominated {
                    tracing::debug!(
                        prompt_id = queued.id,
                        source = %queued.source,
                        "Suppressing duplicate idle message (rate-limited to 5min)"
                    );
                    let _ = queue.mark_processed(queued.id);
                    continue;
                }
                self.last_idle_message_times.insert(queued.source.clone(), now);
            }

            // Skip PTY injection for native extension agents that use plain PTY mode —
            // they poll the queue and deliver messages via their own extension API.
            if target != "all_workers" && native_agents.contains(target.as_str()) {
                continue;
            }

            // Gate PTY injection on pane readiness: Claude Code flushes the PTY
            // input buffer during startup, so text written before readline
            // initialization is silently lost. Wait for output + a 5s grace
            // period before injecting. Teams-mode uses inbox files, not PTY.
            if self.teams.is_none() {
                let pane_target = if target == "supervisor" {
                    self.app.supervisor_name()
                } else {
                    target.as_str()
                };
                if !self.app.mux.pane_ready_for_injection(pane_target) {
                    // Don't ack — the prompt stays in the queue for the next tick.
                    continue;
                }
            }

            let prompt_with_instructions = queued.prompt.clone();
            let preview: String = queued.prompt.chars().take(50).collect();

            // Resolve the queue source to a valid team member name for inbox writes.
            // The source must be a registered team member name for Claude Code to
            // accept it. The supervisor's team name is "supervisor" (not the generated
            // pane name), so we also accept the pane name and map it.
            let inbox_source = if self.teams.is_some() {
                let src = queued.source.as_str();
                if src == "supervisor"
                    || worker_names.iter().any(|w| w == src)
                    || src == super::teams::DIRECTOR_AGENT_NAME
                {
                    queued.source.clone()
                } else if src == supervisor_name {
                    "supervisor".to_string()
                } else {
                    super::teams::DIRECTOR_AGENT_NAME.to_string()
                }
            } else {
                queued.source.clone()
            };

            tracing::info!("Injecting prompt to '{}': {}", target, preview);

            let record_injection = |store: &SqliteEventStore,
                                    prompt_id: i64,
                                    queue_source: &str,
                                    queue_target: &str,
                                    actual_target: &str,
                                    status: &str,
                                    error: Option<String>| {
                let mut meta = serde_json::json!({
                    "prompt_id": prompt_id,
                    "queue_source": queue_source,
                    "queue_target": queue_target,
                    "actual_target": actual_target,
                    "status": status,
                });
                if let Some(err) = error {
                    meta["error"] = serde_json::Value::String(err);
                }
                let summary =
                    format!("Injected queued prompt {prompt_id} to {actual_target} ({status})");
                let ev = Event::new(
                    EventType::SupervisorInjected,
                    EventEntityType::Agent,
                    actual_target,
                    summary,
                )
                .with_metadata(meta);
                let _ = store.record(&ev);
            };

            let mut success = false;
            if target == "all_workers" {
                let workers: Vec<String> = self
                    .app
                    .worker_names()
                    .iter()
                    .filter(|name| {
                        // Skip native extension agents (they self-serve via extension polling).
                        !native_agents.contains(name.as_str())
                    })
                    .cloned()
                    .collect();
                tracing::info!("all_workers target, workers: {:?}", workers);
                if workers.is_empty() {
                    continue;
                }
                let mut any_success = false;
                for name in workers {
                    let inject_result: anyhow::Result<()> = if let Some(ref teams) = self.teams {
                        teams.write_to_inbox(
                            &name,
                            &inbox_source,
                            &prompt_with_instructions,
                            queued.summary.as_deref(),
                            None,
                        )
                    } else {
                        self.app
                            .mux
                            .inject(&name, &prompt_with_instructions)
                            .await
                            .map_err(Into::into)
                    };
                    match inject_result {
                        Ok(_) => {
                            any_success = true;
                            tracing::info!("Injected to worker '{}'", name);
                            if let Some(ref store) = event_store {
                                record_injection(
                                    store,
                                    queued.id,
                                    &queued.source,
                                    &queued.target,
                                    &name,
                                    "ok",
                                    None,
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to inject to '{}': {}", name, e);
                            if let Some(ref store) = event_store {
                                record_injection(
                                    store,
                                    queued.id,
                                    &queued.source,
                                    &queued.target,
                                    &name,
                                    "error",
                                    Some(e.to_string()),
                                );
                            }
                        }
                    }
                }
                success = any_success;
            } else {
                // Resolve target for delivery. For teams, the supervisor's team
                // name is "supervisor". For mux.inject, use the generated pane name.
                let pane_target = if target == "supervisor" {
                    self.app.supervisor_name()
                } else {
                    target.as_str()
                };
                // For teams inbox, map generated pane name back to "supervisor"
                let inbox_target = if pane_target == self.app.supervisor_name() {
                    "supervisor"
                } else {
                    pane_target
                };
                let inject_result: anyhow::Result<()> = if let Some(ref teams) = self.teams {
                    teams.write_to_inbox(
                        inbox_target,
                        &inbox_source,
                        &prompt_with_instructions,
                        queued.summary.as_deref(),
                        None,
                    )
                } else {
                    self.app
                        .mux
                        .inject(pane_target, &prompt_with_instructions)
                        .await
                        .map_err(Into::into)
                };
                match inject_result {
                    Ok(_) => {
                        success = true;
                        // cas-f9e8 telemetry: end-to-end delivery latency
                        // measured from the sender-assigned `created_at` to
                        // the moment the daemon completed the inbox write.
                        // This is the number the P99 SLO tracks.
                        let deliver_ms = (chrono::Utc::now() - queued.created_at)
                            .num_milliseconds();
                        tracing::info!(
                            target: "cas::coordination",
                            stage = "delivered",
                            channel = "prompt_queue",
                            message_id = queued.id,
                            source = %queued.source,
                            target_agent = %pane_target,
                            deliver_ms,
                            "prompt_queue message delivered to inbox"
                        );
                        if let Some(ref store) = event_store {
                            record_injection(
                                store,
                                queued.id,
                                &queued.source,
                                &queued.target,
                                pane_target,
                                "ok",
                                None,
                            );
                        }
                    }
                    Err(e) => {
                        if self.app.mux.get(pane_target).is_none() {
                            // Pane not found — only retry if target is a known
                            // worker/supervisor in this session (it may still be spawning).
                            // Stale messages for workers from previous sessions would
                            // otherwise block the queue forever (peek_all has a limit).
                            let is_current =
                                self.app.worker_names().contains(&pane_target.to_string())
                                    || pane_target == self.app.supervisor_name();
                            if is_current {
                                continue;
                            }
                            tracing::warn!(
                                prompt_id = queued.id,
                                target = pane_target,
                                source = %queued.source,
                                "Abandoning queued prompt for unknown target — \
                                 message will not be delivered"
                            );
                            let _ = queue.mark_processed(queued.id);

                            // Record the drop and notify the supervisor so the
                            // message isn't silently lost.
                            if let Some(ref store) = event_store {
                                record_injection(
                                    store,
                                    queued.id,
                                    &queued.source,
                                    &queued.target,
                                    pane_target,
                                    "abandoned",
                                    Some(format!(
                                        "Target '{}' not found in current session",
                                        pane_target
                                    )),
                                );
                            }

                            // Re-queue to supervisor so the message content isn't lost.
                            // The supervisor can then re-assign the task or re-send.
                            let notice = format!(
                                "<system-notice>\n\
                                 Undelivered message from '{}' to '{}' (target not in session):\n\n\
                                 {}\n\
                                 </system-notice>",
                                queued.source,
                                pane_target,
                                &queued.prompt
                            );
                            let _ = queue.enqueue_with_session(
                                super::teams::DIRECTOR_AGENT_NAME,
                                self.app.supervisor_name(),
                                &notice,
                                &self.session_name,
                            );

                            continue;
                        }
                        tracing::error!("Failed to inject to '{}': {}", pane_target, e);
                        if let Some(ref store) = event_store {
                            record_injection(
                                store,
                                queued.id,
                                &queued.source,
                                &queued.target,
                                pane_target,
                                "error",
                                Some(e.to_string()),
                            );
                        }
                    }
                }
            }

            if success {
                if let Err(e) = queue.mark_processed(queued.id) {
                    tracing::error!("Failed to mark prompt {} as processed: {}", queued.id, e);
                }
            }
        }

        Ok(())
    }

    /// Poll the spawn queue and enqueue individual actions (non-blocking).
    ///
    /// Instead of spawning workers synchronously (which blocks the TUI for seconds),
    /// this converts spawn requests into individual PendingSpawn items that are
    /// processed one-per-tick in the main loop.
    pub(super) fn enqueue_spawn_requests(&mut self) -> anyhow::Result<()> {
        let queue = open_spawn_queue_store(self.app.cas_dir())?;
        let requests = queue.poll(10)?;

        for request in requests {
            tracing::info!("Enqueuing spawn request: {:?}", request.action);
            match request.action {
                SpawnAction::Spawn => {
                    let count = request.count.unwrap_or(1) as usize;
                    let isolate = request.isolate;
                    // cas-2992: deserialize the optional WorkerSpec from the queue row.
                    // Invalid JSON is logged and treated as "no override" so a corrupt row
                    // does not block all subsequent spawns.
                    let spec: Option<cas_mux::WorkerSpec> = request
                        .worker_spec
                        .as_deref()
                        .and_then(|json| match serde_json::from_str(json) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                tracing::warn!(
                                    "spawn queue: invalid worker_spec JSON ({}); using session default",
                                    e
                                );
                                None
                            }
                        });
                    if request.worker_names.is_empty() {
                        self.app.spawning_count += count;
                        for _ in 0..count {
                            self.pending_spawns
                                .push_back(PendingSpawn::Anonymous { isolate, spec: spec.clone() });
                        }
                    } else {
                        self.app.spawning_count += request.worker_names.len();
                        for name in request.worker_names {
                            self.pending_spawns
                                .push_back(PendingSpawn::Named { name, isolate, spec: spec.clone() });
                        }
                    }
                }
                SpawnAction::Shutdown => {
                    self.pending_spawns.push_back(PendingSpawn::Shutdown {
                        count: request.count.map(|c| c as usize),
                        names: request.worker_names,
                        force: request.force,
                    });
                }
                SpawnAction::Respawn => {
                    for name in request.worker_names {
                        self.pending_spawns.push_back(PendingSpawn::Respawn(name));
                    }
                }
            }
        }

        Ok(())
    }

    /// Process pending spawn actions without blocking the main loop.
    ///
    /// Git worktree creation (the slow part) runs on a background thread via
    /// `spawn_blocking`. Only one background spawn runs at a time. Each tick we
    /// either: (a) check if the in-flight spawn finished, or (b) start a new one.
    pub(super) async fn process_pending_spawns(&mut self) {
        // Step 1: Check if in-flight background spawn completed
        if let Some((_, _, ref handle)) = self.spawn_task {
            if !handle.is_finished() {
                return; // Still running, don't start another
            }
            let (pending_name, pending_spec, handle) = self.spawn_task.take().unwrap();
            // Remove from pending workers (boot pane transitions to real pane or disappears)
            self.app.remove_pending_worker(&pending_name);
            match handle.await {
                Ok(Ok(result)) => {
                    // Build per-worker Teams config before finish_worker_spawn adds to worker list
                    let worker_name_for_teams = result.worker_name.clone();
                    let color_idx = self.app.worker_names().len();
                    let teams_config = self.teams.as_ref().map(|t| {
                        use super::teams::TeamsManager;
                        t.spawn_config_for(
                            &worker_name_for_teams,
                            "general-purpose",
                            TeamsManager::color_for_index(color_idx),
                            None,
                        )
                    });
                    // Register TUI color to match the Teams color
                    if let Some(ref tc) = teams_config {
                        crate::ui::theme::register_agent_color(&tc.agent_name, &tc.agent_color);
                    }
                    match self.app.finish_worker_spawn(result, teams_config, pending_spec) {
                        Ok(name) => {
                            tracing::info!("Spawned worker (async): {}", name);
                            // Register new worker with native Agent Teams
                            if let Some(ref teams) = self.teams {
                                let worker_cwd = self
                                    .app
                                    .worktree_manager()
                                    .map(|mgr| mgr.worktree_path_for_worker(&name))
                                    .unwrap_or_else(|| self.app.project_path().to_path_buf());
                                if let Err(e) = teams.add_member(&name, &worker_cwd, color_idx) {
                                    tracing::error!(
                                        "Failed to add worker '{}' to teams: {}",
                                        name,
                                        e
                                    );
                                }
                            }
                            if self.app.record_enabled() {
                                if let Err(e) = self.app.start_recording_for_pane(&name).await {
                                    tracing::error!(
                                        "Failed to start recording for {}: {}",
                                        name,
                                        e
                                    );
                                }
                            }
                            // Notify web viewers of updated pane list
                            if let Some(ref handle) = self.cloud_handle {
                                let mut panes = self.app.worker_names().to_vec();
                                panes.insert(0, self.app.supervisor_name().to_string());
                                handle.send_pane_list(panes);
                            }
                            // Notify GUI and WS clients of new worker pane
                            self.gui_notify_pane_added(&name, cas_mux::PaneKind::Worker);
                        }
                        Err(e) => {
                            crate::telemetry::track(
                                "factory_worker_spawn_result",
                                vec![
                                    ("success", "false"),
                                    ("reason", "finish_worker_spawn_failed"),
                                ],
                            );
                            self.app.set_error(format!("Failed to finish spawn: {e}"));
                        }
                    }
                }
                Ok(Err(e)) => {
                    crate::telemetry::track(
                        "factory_worker_spawn_result",
                        vec![("success", "false"), ("reason", "background_spawn_failed")],
                    );
                    self.app.set_error(format!("Failed to spawn worker: {e}"));
                }
                Err(e) => {
                    crate::telemetry::track(
                        "factory_worker_spawn_result",
                        vec![("success", "false"), ("reason", "spawn_task_panicked")],
                    );
                    self.app.set_error(format!("Spawn task panicked: {e}"));
                }
            }
            self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
            return; // One completion per tick
        }

        // Step 2: No in-flight spawn - pop next action from queue
        let action = match self.pending_spawns.pop_front() {
            Some(a) => a,
            None => return,
        };

        match action {
            PendingSpawn::Anonymous { isolate, spec } => {
                match self.app.prepare_worker_spawn(None, isolate) {
                    Ok(prep) => {
                        let worker_name = prep.worker_name.clone();
                        self.app.add_pending_worker(worker_name.clone(), isolate);
                        self.spawn_task = Some((
                            worker_name,
                            spec,
                            tokio::task::spawn_blocking(move || prep.run()),
                        ));
                    }
                    Err(e) => {
                        crate::telemetry::track(
                            "factory_worker_spawn_result",
                            vec![
                                ("success", "false"),
                                ("reason", "prepare_worker_spawn_failed"),
                            ],
                        );
                        self.app.set_error(format!("Failed to prepare spawn: {e}"));
                        self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
                    }
                }
            }
            PendingSpawn::Named { name, isolate, spec } => {
                match self.app.prepare_worker_spawn(Some(&name), isolate) {
                    Ok(prep) => {
                        let worker_name = prep.worker_name.clone();
                        self.app.add_pending_worker(worker_name.clone(), isolate);
                        self.spawn_task = Some((
                            worker_name,
                            spec,
                            tokio::task::spawn_blocking(move || prep.run()),
                        ));
                    }
                    Err(e) => {
                        crate::telemetry::track(
                            "factory_worker_spawn_result",
                            vec![
                                ("success", "false"),
                                ("reason", "prepare_named_spawn_failed"),
                            ],
                        );
                        self.app
                            .set_error(format!("Failed to prepare spawn '{name}': {e}"));
                        self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
                    }
                }
            }
            PendingSpawn::Shutdown {
                count,
                names,
                force,
            } => {
                // Shutdowns are fast - process synchronously
                // Collect worker names before shutdown for GUI notification
                let workers_to_stop = if !names.is_empty() {
                    names.clone()
                } else {
                    let c = count.unwrap_or(0);
                    if c == 0 {
                        self.app.worker_names().to_vec()
                    } else {
                        self.app.worker_names().iter().take(c).cloned().collect()
                    }
                };
                if self.app.record_enabled() {
                    for name in &workers_to_stop {
                        let _ = self.app.stop_recording_for_pane(name).await;
                    }
                }
                // Track shut-down workers so their queued messages are dropped
                for name in &workers_to_stop {
                    self.dead_workers.insert(name.clone());
                }
                if let Err(e) = self.app.shutdown_workers(count, &names, force) {
                    let target = if !names.is_empty() {
                        names.join(", ")
                    } else if let Some(c) = count {
                        if c == 0 {
                            "all workers".to_string()
                        } else {
                            format!("{c} worker(s)")
                        }
                    } else {
                        "all workers".to_string()
                    };
                    self.app
                        .set_error(format!("Failed to shutdown {target}: {e}"));
                    tracing::error!("Failed to shutdown {}: {}", target, e);
                } else {
                    // Remove shut-down workers from native Agent Teams
                    if let Some(ref teams) = self.teams {
                        for name in &workers_to_stop {
                            let _ = teams.remove_member(name);
                        }
                    }
                    // Notify GUI and WS clients that panes were removed
                    for name in &workers_to_stop {
                        self.gui_notify_pane_removed(name);
                    }
                }
            }
            PendingSpawn::Respawn(name) => {
                // Build per-worker Teams config for the respawned worker
                let teams_config = self.teams.as_ref().map(|t| {
                    use super::teams::TeamsManager;
                    let color_idx = self.app.worker_names().len();
                    t.spawn_config_for(
                        &name,
                        "general-purpose",
                        TeamsManager::color_for_index(color_idx),
                        None,
                    )
                });
                // Register TUI color to match the Teams color
                if let Some(ref tc) = teams_config {
                    crate::ui::theme::register_agent_color(&tc.agent_name, &tc.agent_color);
                }
                // Respawn reuses existing worktree - fast enough to run synchronously
                match self.app.respawn_worker(&name, teams_config) {
                    Ok(()) => {
                        if self.app.record_enabled() {
                            if let Err(e) = self.app.start_recording_for_pane(&name).await {
                                tracing::error!(
                                    "Failed to start recording for respawned {}: {}",
                                    name,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        self.app.set_error(format!("Failed to respawn {name}: {e}"));
                    }
                }
            }
            PendingSpawn::Shell { name, shell } => {
                let cwd = self.app.project_path().to_path_buf();
                match self.app.mux.add_shell(&name, cwd, shell.as_deref()) {
                    Ok(_) => {
                        tracing::info!("Shell pane '{}' spawned", name);
                        self.gui_notify_pane_added(&name, cas_mux::PaneKind::Shell);
                    }
                    Err(e) => {
                        self.app
                            .set_error(format!("Failed to spawn shell '{name}': {e}"));
                        tracing::error!("Failed to spawn shell '{}': {}", name, e);
                    }
                }
            }
            PendingSpawn::KillShell { name } => match self.app.mux.remove_shell(&name) {
                Ok(()) => {
                    tracing::info!("Shell pane '{}' killed", name);
                    self.gui_notify_pane_removed(&name);
                }
                Err(e) => {
                    self.app
                        .set_error(format!("Failed to kill shell '{name}': {e}"));
                    tracing::error!("Failed to kill shell '{}': {}", name, e);
                }
            },
        }
    }

    /// Process pending reminders (time-based and event-based)
    ///
    /// Called during the 2-second refresh cycle with the events detected in this tick.
    /// Time-based reminders fire when trigger_at <= now.
    /// Event-based reminders fire when a matching DirectorEvent is detected.
    /// Delivery uses both the supervisor notification queue (for structured data / web UI)
    /// and the prompt queue (for PTY injection into the supervisor's session).
    pub(super) fn process_reminders(&self, events: &[crate::ui::factory::director::DirectorEvent]) {
        use crate::store::{
            open_prompt_queue_store, open_reminder_store, open_supervisor_queue_store,
        };

        let reminder_store = match open_reminder_store(self.app.cas_dir()) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("Failed to open reminder store: {}", e);
                return;
            }
        };

        // Expire stale reminders
        if let Err(e) = reminder_store.expire_stale() {
            tracing::error!("Failed to expire stale reminders: {}", e);
        }

        // Check time-based reminders
        let due_reminders = match reminder_store.get_due_time_reminders() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to get due reminders: {}", e);
                Vec::new()
            }
        };

        let supervisor_queue = if !due_reminders.is_empty() || !events.is_empty() {
            open_supervisor_queue_store(self.app.cas_dir()).ok()
        } else {
            None
        };

        // Open prompt queue for PTY injection of fired reminders
        let prompt_queue = if !due_reminders.is_empty() || !events.is_empty() {
            open_prompt_queue_store(self.app.cas_dir()).ok()
        } else {
            None
        };

        let agent_id_to_name = &self.app.director_data().agent_id_to_name;

        for reminder in &due_reminders {
            fire_reminder(
                reminder,
                &reminder_store,
                &supervisor_queue,
                &prompt_queue,
                &self.session_name,
                agent_id_to_name,
                None,
            );
        }

        // Check event-based reminders against detected events
        for event in events {
            let event_type = event.event_type();
            let candidates = match reminder_store.get_event_reminders(event_type) {
                Ok(r) => r,
                Err(_) => continue,
            };

            for reminder in &candidates {
                if matches_event_filter(reminder, event) {
                    fire_reminder(
                        reminder,
                        &reminder_store,
                        &supervisor_queue,
                        &prompt_queue,
                        &self.session_name,
                        agent_id_to_name,
                        Some(event),
                    );
                }
            }
        }
    }

    /// Handle epic state change
    ///
    /// Manages git branches when epic state transitions:
    /// - Started: Creates epic branch, workers branch from it
    /// - Completed: Merges worker branches to epic branch
    pub(super) async fn handle_epic_change(
        &mut self,
        change: EpicStateChange,
    ) -> anyhow::Result<()> {
        match change {
            EpicStateChange::Started {
                epic_id,
                epic_title,
                previous_state,
            } => {
                // Update terminal title with the new epic
                set_terminal_title(self.app.project_path(), Some(&epic_title));

                // Create epic branch when transitioning from Idle
                if matches!(previous_state, crate::ui::factory::app::EpicState::Idle) {
                    match self.app.create_epic_branch(&epic_title) {
                        Ok(branch) => {
                            tracing::info!(
                                "EPIC {} started - created branch '{}' for workers",
                                epic_id,
                                branch
                            );
                        }
                        Err(e) => {
                            tracing::error!("Failed to create epic branch for {}: {}", epic_id, e);
                            self.app
                                .set_error(format!("Failed to create epic branch: {e}"));
                        }
                    }
                } else if self.resumed_epic_ids.insert(epic_id.clone()) {
                    tracing::info!(
                        "EPIC {} started (resuming) - using existing branch",
                        epic_id
                    );
                }
            }

            EpicStateChange::Completed {
                epic_id,
                epic_title,
            } => {
                // Update terminal title to show no active epic
                set_terminal_title(self.app.project_path(), None);

                // Merge worker branches to epic branch
                tracing::info!(
                    "EPIC {} ({}) completed - merging worker branches",
                    epic_id,
                    epic_title
                );

                match self.app.merge_workers_to_epic() {
                    Ok(results) => {
                        let success_count = results.iter().filter(|(_, ok, _)| *ok).count();
                        let fail_count = results.len() - success_count;

                        if fail_count > 0 {
                            let failures: Vec<_> = results
                                .iter()
                                .filter(|(_, ok, _)| !ok)
                                .map(|(name, _, msg)| {
                                    format!(
                                        "{}: {}",
                                        name,
                                        msg.as_deref().unwrap_or("unknown error")
                                    )
                                })
                                .collect();
                            tracing::warn!(
                                "EPIC {} merge: {}/{} workers merged. Failures: {:?}",
                                epic_id,
                                success_count,
                                results.len(),
                                failures
                            );
                            self.app.set_error(format!(
                                "Epic merge: {fail_count} worker(s) failed to merge"
                            ));
                        } else {
                            tracing::info!(
                                "EPIC {} merge complete: all {} workers merged",
                                epic_id,
                                success_count
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to merge workers for EPIC {}: {}", epic_id, e);
                        self.app.set_error(format!("Failed to merge workers: {e}"));
                    }
                }

                // Note: Worker branch cleanup is handled via /factory-merge-epic skill
                // to give supervisor control over the cleanup process
            }
        }
        Ok(())
    }
}

/// Fire a reminder by delivering it to both the notification queue
/// (for web UI / structured data) and the prompt queue (for PTY injection).
///
/// `agent_id_to_name` maps agent UUIDs to pane names that the prompt queue
/// can route to. Falls back to `"supervisor"` when the target agent ID is
/// not found in the map.
///
/// `triggering_event` is the DirectorEvent that caused this reminder to fire
/// (only set for event-based reminders). Its context is included in the
/// delivered prompt so the recipient knows what happened.
fn fire_reminder(
    reminder: &cas_store::Reminder,
    reminder_store: &std::sync::Arc<dyn cas_store::ReminderStore>,
    supervisor_queue: &Option<std::sync::Arc<dyn cas_store::SupervisorQueueStore>>,
    prompt_queue: &Option<std::sync::Arc<dyn cas_store::PromptQueueStore>>,
    session_name: &str,
    agent_id_to_name: &std::collections::HashMap<String, String>,
    triggering_event: Option<&crate::ui::factory::director::DirectorEvent>,
) {
    // Build event JSON for persistence
    let event_json = triggering_event.map(|e| {
        serde_json::json!({
            "event_type": e.event_type(),
            "data": e.to_json(),
            "description": e.description(),
        })
    });

    // Mark as fired first to prevent double-fire on next tick
    if let Err(e) = reminder_store.mark_fired(reminder.id, event_json.as_ref()) {
        tracing::error!("Failed to mark reminder {} as fired: {}", reminder.id, e);
        return;
    }

    let mut payload = serde_json::json!({
        "reminder_id": reminder.id,
        "message": reminder.message,
        "target_id": reminder.target_id,
        "trigger_type": reminder.trigger_type.to_string(),
    });
    if let Some(event) = triggering_event {
        payload["event_type"] = serde_json::Value::String(event.event_type().to_string());
        payload["event"] = event.to_json();
    }
    let payload = payload.to_string();

    // Enqueue to notification queue (for web UI / structured data).
    // Notify the owner so they know their reminder fired.
    if let Some(queue) = supervisor_queue {
        if let Err(e) = queue.notify(
            &reminder.owner_id,
            "reminder_fired",
            &payload,
            cas_store::NotificationPriority::Normal,
        ) {
            tracing::error!("Failed to enqueue reminder notification: {}", e);
        }
    }

    // Enqueue to prompt queue for PTY injection into the target agent's session.
    // Resolve the target agent UUID to its pane name. process_prompt_queue also
    // resolves the logical name "supervisor" to the actual pane name, so we use
    // that as fallback when the target ID isn't in the map.
    if let Some(queue) = prompt_queue {
        let target = agent_id_to_name
            .get(&reminder.target_id)
            .map(|s| s.as_str())
            .unwrap_or("supervisor");

        // Include triggering event context for event-based reminders
        let prompt = match triggering_event {
            Some(event) => format!(
                "Reminder #{}: {} (triggered by: {})",
                reminder.id,
                reminder.message,
                event.description()
            ),
            None => format!("Reminder #{}: {}", reminder.id, reminder.message),
        };

        if let Err(e) =
            queue.enqueue_with_session(&reminder.owner_id, target, &prompt, session_name)
        {
            tracing::error!("Failed to enqueue reminder prompt: {}", e);
        } else {
            tracing::info!(
                "Fired reminder #{} → {} ({}): {}",
                reminder.id,
                target,
                reminder.target_id,
                reminder.message
            );
        }
    }
}

/// Check if a reminder's event filter matches a detected DirectorEvent.
///
/// Uses JSON subset matching: every key-value in the filter must appear
/// in the event's JSON representation. An empty or missing filter matches
/// any event of the correct type.
fn matches_event_filter(
    reminder: &cas_store::Reminder,
    event: &crate::ui::factory::director::DirectorEvent,
) -> bool {
    let filter = match &reminder.trigger_filter {
        Some(f) => f,
        None => return true, // No filter = match any event of this type
    };

    let event_data = event.to_json();

    match (filter.as_object(), event_data.as_object()) {
        (Some(filter_obj), Some(event_obj)) => {
            for (key, expected) in filter_obj {
                match event_obj.get(key) {
                    Some(actual) if actual == expected => continue,
                    _ => return false,
                }
            }
            true
        }
        _ => false,
    }
}

fn is_exact_agent_name_match(agent: &AgentSummary, worker_name: &str) -> bool {
    agent.name == worker_name
}

#[cfg(test)]
mod tests {
    use super::is_exact_agent_name_match;
    use crate::ui::factory::daemon::FactoryDaemon;
    use crate::ui::factory::director::AgentSummary;
    use cas_types::AgentStatus;

    #[test]
    fn test_agent_match_is_exact_not_substring() {
        let worker_10 = AgentSummary {
            id: "agent-10".to_string(),
            name: "worker-10".to_string(),
            status: AgentStatus::Active,
            current_task: None,
            latest_activity: None,
            last_heartbeat: None,
        };

        assert!(
            !is_exact_agent_name_match(&worker_10, "worker-1"),
            "worker-1 must not match worker-10"
        );
        assert!(is_exact_agent_name_match(&worker_10, "worker-10"));
    }

    #[test]
    fn test_is_idle_message_matches_stock_heartbeats() {
        // Bare stock phrases.
        assert!(FactoryDaemon::is_idle_message("Standing by."));
        assert!(FactoryDaemon::is_idle_message("Ready for task."));
        assert!(FactoryDaemon::is_idle_message("Ready for tasks."));
        assert!(FactoryDaemon::is_idle_message("Awaiting instructions."));
        assert!(FactoryDaemon::is_idle_message("Awaiting task."));
        assert!(FactoryDaemon::is_idle_message("Waiting for work."));
        assert!(FactoryDaemon::is_idle_message("No task assigned."));
        // Case-insensitive and leading whitespace tolerant.
        assert!(FactoryDaemon::is_idle_message("  STANDING BY."));
        assert!(FactoryDaemon::is_idle_message("standing by for further direction"));
    }

    /// Regression for cas-f9e8: the old unanchored substring filter silently
    /// dropped any message containing the literal word "idle" or an idle
    /// phrase buried mid-message. These are real status/debug messages that
    /// must flow through to the supervisor.
    #[test]
    fn test_is_idle_message_does_not_match_status_reports_containing_idle_words() {
        // "idle" as a bare substring — the old filter would have dropped this.
        assert!(!FactoryDaemon::is_idle_message(
            "Fix 1 for the WorkerIdle debounce race is in HEAD."
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "the idle detector now requires two consecutive ticks"
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "I am idle, waiting for work." // starts with "I am", not a stock phrase
        ));
        // Idle phrase buried mid-message, not at the start.
        assert!(!FactoryDaemon::is_idle_message(
            "Task cas-1234 closed. Standing by for the next assignment now."
        ));
        // Diagnostic message that previously matched "mcp tools unavailable"
        // as a substring — that phrase has been removed from the filter.
        assert!(!FactoryDaemon::is_idle_message(
            "MCP tools unavailable — falling back to direct sqlite; see bugfix memory."
        ));
        // Real work reports.
        assert!(!FactoryDaemon::is_idle_message(
            "COMPLETED task cas-1234. Commit: abc123."
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "Blocked: cannot compile due to missing dep."
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "Fixed the bug in parser.rs, tests pass."
        ));
    }

    /// Regression for cas-f9e8: very long messages that happen to mention an
    /// idle phrase must never be classified as idle heartbeats, because the
    /// daemon silently drops rate-limited matches without delivering them.
    #[test]
    fn test_is_idle_message_rejects_long_messages_even_when_starting_with_idle_phrase() {
        let long_report = format!(
            "Standing by. {}",
            "x".repeat(320) // pushes total length past MAX_IDLE_LEN
        );
        assert!(
            !FactoryDaemon::is_idle_message(&long_report),
            "long messages must never be treated as idle heartbeats even when they \
             start with a stock phrase — idle filter silently drops matches, so a \
             false positive here would lose the entire report"
        );
    }
}
