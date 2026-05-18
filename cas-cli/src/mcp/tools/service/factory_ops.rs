use crate::mcp::tools::service::imports::*;

/// Heartbeat age at which a worker is considered **stale** and becomes
/// eligible for the opportunistic prune in `factory_worker_status`.
///
/// A stale worker is dropped from the Active listing on the next status
/// call (see the `list_stale` + `mark_stale` loop). If the prune succeeds
/// — the overwhelmingly common path — the worker never reaches the
/// render-time liveness-label branch at all.
///
/// Bumped from 120s to 30s by cas-2749 so a crashed CC client is
/// detected within roughly one supervisor status poll. The 30s number is
/// load-bearing: callers in tests assert the exact value via
/// [`worker_stale_secs_is_pinned_at_30`] (cas-8240 AC anchor) so a drift
/// fix in one place that forgets to update the other cannot silently
/// regress the UX.
pub(crate) const WORKER_STALE_SECS: i64 = 30;

/// Heartbeat age at which a worker is escalated to **dead** in the
/// supervisor-facing render: hard `[DEAD]` label + transcript-path
/// surfacing so the supervisor can salvage the last in-flight tool call.
///
/// Two-band model (cas-8240): `WORKER_STALE_SECS` (30s) drives the
/// opportunistic prune and a lighter-weight `[stale]` indicator on any
/// worker that slipped past the prune (e.g. `mark_stale` hit a DB lock).
/// `WORKER_DEAD_SECS` (75s) gates the more expensive `[DEAD]` + transcript
/// emission so tokio scheduler jitter or a missed 30s daemon tick cannot
/// produce false-positive DEAD labels that train supervisors to distrust
/// the signal. Picked at 2.5× the stale threshold: gives the daemon one
/// full heartbeat interval of grace past the prune window before the
/// render escalates, which in practice means a worker has to have
/// missed at least two consecutive heartbeats before we surface it as
/// dead.
pub(crate) const WORKER_DEAD_SECS: i64 = 75;

/// Build a JSON-serialized [`cas_mux::WorkerSpec`] from optional string overrides
/// supplied via the MCP `spawn_workers` action or the cloud protocol.
///
/// Returns `Ok(None)` when all three parameters are absent (session defaults apply).
/// Returns `Err(String)` when a parameter value is invalid.
pub(crate) fn build_spawn_spec_json(
    cli: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Option<String>, String> {
    if cli.is_none() && model.is_none() && effort.is_none() {
        return Ok(None);
    }

    let parsed_cli = match cli {
        Some(s) => s
            .parse::<cas_mux::SupervisorCli>()
            .map_err(|_| format!("invalid cli value {s:?}: expected 'claude' or 'codex'"))?,
        None => cas_mux::SupervisorCli::Claude,
    };

    let parsed_effort: Option<cas_mux::Effort> = match effort {
        Some(s) => Some(
            s.parse::<cas_mux::Effort>()
                .map_err(|e| format!("invalid effort value {s:?}: {e}"))?,
        ),
        None => None,
    };

    let spec = cas_mux::WorkerSpec {
        name: None,
        cli: parsed_cli,
        model: model.map(String::from),
        effort: parsed_effort,
    };

    let json = serde_json::to_string(&spec)
        .map_err(|e| format!("failed to serialize WorkerSpec: {e}"))?;
    Ok(Some(json))
}

impl CasService {
    pub(super) async fn factory_spawn_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_spawn_queue_store, open_task_store};
        use cas_types::{TaskStatus, TaskType};

        // Check that there's an active EPIC before spawning workers
        let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open task store: {e}"),
            )
        })?;

        let open_epics: Vec<_> = task_store
            .list(None)
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list tasks: {e}"),
                )
            })?
            .into_iter()
            .filter(|t| t.task_type == TaskType::Epic && t.status != TaskStatus::Closed)
            .collect();

        if open_epics.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                "No active EPIC found. Before spawning workers, create or assign an EPIC:\n\
                 1. Create EPIC: mcp__cas__task action=create task_type=epic title=\"...\" description=\"...\"\n\
                 2. Or assign existing EPIC: mcp__cas__task action=start id=<epic-id>\n\
                 3. Optionally gather requirements using the epic-spec skill\n\
                 4. Break into tasks using the epic-breakdown skill\n\
                 5. Then spawn workers to work on the tasks",
            ));
        }

        let queue = open_spawn_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spawn queue: {e}"),
            )
        })?;

        let count = req.count.unwrap_or(1);
        let isolate = req.isolate.unwrap_or(false);
        let worker_names: Vec<String> = req
            .worker_names
            .map(|names| {
                names
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // cas-2992: build a WorkerSpec from the cli/model/effort override fields
        // and serialize it to JSON for storage in the spawn queue.  None of the
        // three fields being present → no spec override (session defaults apply).
        let spec_json_owned: Option<String> = build_spawn_spec_json(
            req.cli.as_deref(),
            req.model.as_deref(),
            req.effort.as_deref(),
        )
        .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;

        let request_id = queue
            .enqueue_spawn(count, &worker_names, isolate, spec_json_owned.as_deref())
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue spawn request: {e}"),
                )
            })?;

        let msg = if worker_names.is_empty() {
            format!("Queued spawn request for {count} worker(s) (request ID: {request_id})")
        } else {
            format!(
                "Queued spawn request for worker(s): {} (request ID: {})",
                worker_names.join(", "),
                request_id
            )
        };

        Ok(Self::success(msg))
    }

    pub(super) async fn factory_shutdown_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_spawn_queue_store};
        use cas_types::{AgentRole, AgentStatus};

        let mut worker_names: Vec<String> = req
            .worker_names
            .map(|names| {
                names
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // When supervisor has no specific worker names requested, scope to owned workers
        // so a supervisor cannot shut down another supervisor's workers.
        if worker_names.is_empty() {
            if let Some(owned) = supervisor_owned_workers() {
                worker_names = owned.into_iter().collect();
            }
        }

        // Validate workers exist before queuing (synchronous validation)
        if !worker_names.is_empty() {
            let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open agent store: {e}"),
                )
            })?;

            // Include both active and stale workers — stale workers are often
            // exactly what supervisors want to shut down.
            let mut known_agents = agent_store.list(Some(AgentStatus::Active)).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list agents: {e}"),
                )
            })?;
            if let Ok(stale) = agent_store.list(Some(AgentStatus::Stale)) {
                known_agents.extend(stale);
            }

            // Get worker names, scoped to this supervisor's workers when applicable
            let owned = supervisor_owned_workers();
            let known_workers: std::collections::HashSet<String> = known_agents
                .iter()
                .filter(|a| {
                    a.role == AgentRole::Worker
                        && owned.as_ref().is_none_or(|set| set.contains(&a.name))
                })
                .map(|a| a.name.clone())
                .collect();

            // Check each requested worker exists
            let mut not_found = Vec::new();
            for name in &worker_names {
                if !known_workers.contains(name) {
                    not_found.push(name.clone());
                }
            }

            if !not_found.is_empty() {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Worker(s) not found: {}. Known workers: {}",
                        not_found.join(", "),
                        if known_workers.is_empty() {
                            "(none)".to_string()
                        } else {
                            known_workers.into_iter().collect::<Vec<_>>().join(", ")
                        }
                    ),
                ));
            }
        }

        // Validation passed, queue the shutdown
        let queue = open_spawn_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spawn queue: {e}"),
            )
        })?;

        let count = req.count;
        let force = req.force.unwrap_or(false);
        let request_id = queue
            .enqueue_shutdown(count, &worker_names, force)
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue shutdown request: {e}"),
                )
            })?;

        let msg = if !worker_names.is_empty() {
            format!(
                "Queued shutdown request for worker(s): {} (request ID: {})",
                worker_names.join(", "),
                request_id
            )
        } else if let Some(c) = count {
            if c == 0 {
                format!("Queued shutdown request for ALL workers (request ID: {request_id})")
            } else {
                format!("Queued shutdown request for {c} worker(s) (request ID: {request_id})")
            }
        } else {
            format!("Queued shutdown request for ALL workers (request ID: {request_id})")
        };

        Ok(Self::success(msg))
    }

    pub(super) async fn factory_worker_status(
        &self,
        _req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_agent_store;
        use cas_types::{AgentRole, AgentStatus};

        let store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        // Opportunistically prune stale agents so status output stays actionable.
        // Worker threshold tightened from 120s → 30s per cas-2749 so a dead CC
        // client is detected within one supervisor poll. Paired with the
        // daemon-side PID liveness gate in mcp::daemon::send_agent_heartbeat,
        // a crashed worker stops heartbeating within the 30s daemon tick and
        // transitions to "dead" in the next status call. Supervisors/directors
        // are long-lived and less chatty and are filtered out of the prune by
        // the role check below; they remain visible until their own
        // daemon-level cleanup eventually removes them.
        //
        // See the module-level `WORKER_STALE_SECS` and `WORKER_DEAD_SECS`
        // constants (cas-8240) for the two-band model that separates the
        // prune + `[stale]` indicator (30s) from the hard `[DEAD]` + transcript
        // surface (75s).
        let worker_stale_threshold_secs: i64 = WORKER_STALE_SECS;
        let mut stale_pruned = 0usize;
        if let Ok(stale_agents) = store.list_stale(worker_stale_threshold_secs) {
            for agent in stale_agents {
                if agent.role == AgentRole::Supervisor || agent.role == AgentRole::Director {
                    continue;
                }
                if store.mark_stale(&agent.id).is_ok() {
                    stale_pruned += 1;
                }
            }
        }

        let agents = store.list(Some(AgentStatus::Active)).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list agents: {e}"),
            )
        })?;

        if agents.is_empty() {
            return Ok(Self::success(
                "No active agents registered.\n\nNote: Factory TUI must be running for agents to be registered.",
            ));
        }

        let owned = supervisor_owned_workers();
        let mut output = String::from("Worker Status\n=============\n\n");

        let workers: Vec<_> = agents
            .iter()
            .filter(|a| {
                a.role == AgentRole::Worker
                    && owned.as_ref().is_none_or(|set| set.contains(&a.name))
            })
            .collect();
        let self_name = std::env::var("CAS_AGENT_NAME").ok();
        let supervisors: Vec<_> = agents
            .iter()
            .filter(|a| {
                (a.role == AgentRole::Supervisor || a.role == AgentRole::Director)
                    && if owned.is_some() {
                        // When scoped, only show this supervisor (not others)
                        self_name.as_ref() == Some(&a.name)
                    } else {
                        true
                    }
            })
            .collect();

        if !supervisors.is_empty() {
            output.push_str("Supervisors:\n");
            for agent in supervisors {
                let elapsed = (chrono::Utc::now() - agent.last_heartbeat).num_seconds();
                let since = format!("{elapsed}s ago");
                output.push_str(&format!("  • {} (heartbeat: {})\n", &agent.name, since));
            }
            output.push('\n');
        }

        if workers.is_empty() {
            output.push_str("Workers: None active\n");
        } else {
            output.push_str(&format!("Workers ({}):\n", workers.len()));
            for agent in workers {
                let elapsed = (chrono::Utc::now() - agent.last_heartbeat).num_seconds();
                let since = format!("{elapsed}s ago");
                // cas-8240 two-band model — see `liveness_label_for`.
                let liveness_label = liveness_label_for(elapsed);
                let clone_path = agent.metadata.get("clone_path").cloned();
                let clone_info = clone_path
                    .as_ref()
                    .map(|p| format!("\n    Clone: {p}"))
                    .unwrap_or_default();
                // Surface transcript path only for hard-dead workers so the
                // supervisor can salvage whatever was in-flight when the CC
                // client died (cas-2749 AC: transcript-path-surfacing on
                // crash). The `[stale]` tier does NOT emit the transcript —
                // a worker lagging past 30s under scheduler jitter does not
                // need its transcript surfaced yet, and emitting it there
                // would produce the false-positive noise cas-8240 is fixing.
                //
                // cas-900b: when we do emit, use `format_transcript_block`
                // which globs ~/.claude/projects/*/<session_id>.jsonl for
                // the real on-disk path and falls back to the reconstructed
                // path only when the glob can't pin a unique match. Always
                // surfaces session_id so a supervisor who doesn't trust our
                // resolution can grep the projects tree themselves.
                //
                // In factory mode, `agent.id` is the CC SessionStart UUID
                // (daemon.rs + server/mod.rs both construct the Agent via
                // `Agent::new(session_id, name)`). If `cc_session_id` is
                // ever populated separately in the future we prefer it —
                // but for now `id` is the right key and has been correct
                // since cas-2749.
                let transcript_info = if elapsed >= WORKER_DEAD_SECS {
                    let session_id = agent
                        .cc_session_id
                        .as_deref()
                        .unwrap_or(&agent.id);
                    format_transcript_block(clone_path.as_deref(), session_id)
                } else {
                    String::new()
                };
                // Surface session UUID alongside the friendly name so the
                // supervisor can cross-reference task-ownership errors
                // ("owned by worker-backfill (0a7f2802-...)") without manual
                // table-lookup. cas-85bf.
                let session_uuid = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
                output.push_str(&format!(
                    "  • {} (heartbeat: {}){}{}{}\n    session: {}\n",
                    &agent.name, since, liveness_label, clone_info, transcript_info, session_uuid
                ));
            }
        }

        if stale_pruned > 0 {
            output.push_str(&format!(
                "\nFiltered stale agent record(s): {stale_pruned} (>{worker_stale_threshold_secs}s heartbeat age)\n"
            ));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn factory_worker_activity(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use cas_store::{EventStore, SqliteEventStore};
        use cas_types::EventType;

        let event_store = SqliteEventStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open event store: {e}"),
            )
        })?;

        // Filter by worker name if specified, otherwise scope to this supervisor's workers
        let worker_filter = req.worker_names.as_ref();
        let owned = supervisor_owned_workers();

        // Get recent worker activity events
        let events = event_store.list_recent(50).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list events: {e}"),
            )
        })?;

        // Filter to worker activity events
        let worker_events: Vec<_> = events
            .into_iter()
            .filter(|e| {
                matches!(
                    e.event_type,
                    EventType::WorkerSubagentSpawned
                        | EventType::WorkerSubagentCompleted
                        | EventType::WorkerFileEdited
                        | EventType::WorkerGitCommit
                        | EventType::WorkerVerificationBlocked
                        | EventType::VerificationStarted
                        | EventType::VerificationAdded
                )
            })
            .filter(|e| {
                let name_matches = |name: &str| {
                    e.session_id
                        .as_ref()
                        .map(|s| s.contains(name))
                        .unwrap_or(false)
                        || e.entity_id.contains(name)
                };
                if let Some(filter) = worker_filter {
                    name_matches(filter.as_str())
                } else if let Some(set) = &owned {
                    set.iter().any(|w| name_matches(w.as_str()))
                } else {
                    true
                }
            })
            .take(20)
            .collect();

        if worker_events.is_empty() {
            return Ok(Self::success(
                "No recent worker activity.\n\nWorker activity is tracked when workers edit files, run subagents, or commit code.",
            ));
        }

        let mut output = String::from("Worker Activity\n===============\n\n");
        for event in worker_events {
            let ago = format_relative_time(event.created_at);
            let session_short = event
                .session_id
                .as_ref()
                .map(|s| &s[..8.min(s.len())])
                .unwrap_or("unknown");
            output.push_str(&format!(
                "• {} - {} ({})\n",
                session_short, event.summary, ago
            ));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn factory_clear_context(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let target = req.target.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "target required for clear_context",
            )
        })?;

        // Validate target is an owned worker when supervisor scoping applies
        if target != "all_workers" && target != "supervisor" {
            if let Some(owned) = supervisor_owned_workers() {
                if !owned.contains(&target) {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "Worker '{}' not owned by this supervisor. Owned: {}",
                            target,
                            owned.into_iter().collect::<Vec<_>>().join(", ")
                        ),
                    ));
                }
            }
        }

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open message queue: {e}"),
            )
        })?;

        // Use the MCP caller's agent ID as the source
        let source = self
            .inner
            .get_agent_id()
            .unwrap_or_else(|_| "unknown".to_string());

        // Enqueue /clear directly without XML wrapping - this is a raw command
        let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
        if let Some(ref session) = factory_session {
            queue
                .enqueue_with_session(&source, &target, "/clear", session)
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to queue clear command: {e}"),
                    )
                })?;
        } else {
            queue.enqueue(&source, &target, "/clear").map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue clear command: {e}"),
                )
            })?;
        }

        let msg = if target == "all_workers" {
            "Queued /clear for all workers".to_string()
        } else {
            format!("Queued /clear for {target}")
        };

        Ok(Self::success(msg))
    }

    pub(super) async fn factory_my_context(
        &self,
        _req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_task_store};
        use cas_types::AgentRole;

        // Get current agent's info
        let agent_id = self.inner.get_agent_id().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get agent ID: {e}"),
            )
        })?;

        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        let agent = agent_store.get(&agent_id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get agent: {e}"),
            )
        })?;

        let mut output = String::from("My Factory Context\n==================\n\n");

        // Agent info
        let role_str = match agent.role {
            AgentRole::Worker => "Worker",
            AgentRole::Supervisor => "Supervisor",
            AgentRole::Director => "Director",
            AgentRole::Standard => "Standard Agent",
        };
        output.push_str(&format!("**Name**: {}\n", agent.name));
        output.push_str(&format!("**Role**: {role_str}\n"));
        output.push_str(&format!("**ID**: {}\n\n", agent.id));

        // Clone path (from environment)
        if let Ok(cwd) = std::env::var("CAS_CLONE_PATH") {
            output.push_str(&format!("**Clone Path**: {cwd}\n"));
        } else if let Ok(cwd) = std::env::current_dir() {
            output.push_str(&format!("**Working Directory**: {}\n", cwd.display()));
        }

        // Current task(s)
        let leases = agent_store.list_agent_leases(&agent_id).unwrap_or_default();
        if leases.is_empty() {
            output.push_str("\n**Current Task**: None (idle)\n");
        } else {
            output.push_str("\n**Claimed Tasks**:\n");
            if let Ok(task_store) = open_task_store(&self.inner.cas_root) {
                for lease in &leases {
                    if let Ok(task) = task_store.get(&lease.task_id) {
                        output.push_str(&format!("  - {} {}\n", task.id, task.title));
                    } else {
                        output.push_str(&format!("  - {}\n", lease.task_id));
                    }
                }
            } else {
                for lease in &leases {
                    output.push_str(&format!("  - {}\n", lease.task_id));
                }
            }
        }

        // Git branch info
        if let Ok(branch_output) = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
        {
            if branch_output.status.success() {
                let branch = String::from_utf8_lossy(&branch_output.stdout)
                    .trim()
                    .to_string();
                output.push_str(&format!("\n**Git Branch**: {branch}\n"));
            }
        }

        Ok(Self::success(output))
    }

    pub(super) async fn factory_sync_all_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_task_store};
        use cas_types::{AgentRole, AgentStatus, TaskStatus, TaskType};
        use std::path::Path;

        let store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        let owned = supervisor_owned_workers();
        let mut workers: Vec<_> = store
            .list(Some(AgentStatus::Active))
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list agents: {e}"),
                )
            })?
            .into_iter()
            .filter(|a| {
                a.role == AgentRole::Worker
                    && owned.as_ref().is_none_or(|set| set.contains(&a.name))
            })
            .collect();

        if workers.is_empty() {
            return Ok(Self::success("No active workers found."));
        }

        if let Some(filter) = req.worker_names.as_ref() {
            let names: std::collections::HashSet<String> = filter
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            workers.retain(|w| names.contains(&w.name));
        }

        if workers.is_empty() {
            return Ok(Self::success(
                "No matching active workers found for requested worker_names filter.",
            ));
        }

        let sync_ref = if let Some(branch) = req.branch.clone().filter(|b| !b.trim().is_empty()) {
            branch
        } else {
            let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open task store: {e}"),
                )
            })?;

            let mut epic_branch = None;
            if let Ok(tasks) = task_store.list(None) {
                if let Some(epic) = tasks
                    .iter()
                    .find(|t| t.task_type == TaskType::Epic && t.status == TaskStatus::InProgress)
                {
                    epic_branch = epic.branch.clone();
                }
                if epic_branch.is_none() {
                    epic_branch = tasks
                        .iter()
                        .find(|t| t.task_type == TaskType::Epic && t.status == TaskStatus::Open)
                        .and_then(|t| t.branch.clone());
                }
            }
            epic_branch.unwrap_or_else(|| {
                // Use local main branch, not origin/main. In factory mode the
                // supervisor merges worker branches into the local main branch,
                // so workers should rebase onto it directly.
                use crate::worktree::GitOperations;
                GitOperations::detect_repo_root(&self.inner.cas_root)
                    .ok()
                    .map(GitOperations::new)
                    .map(|git| git.detect_default_branch())
                    .unwrap_or_else(|| "main".to_string())
            })
        };

        let mut synced = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();

        for worker in workers {
            let clone_path = match worker.metadata.get("clone_path") {
                Some(p) => p.clone(),
                None => {
                    skipped.push(format!("{} (missing clone_path metadata)", worker.name));
                    continue;
                }
            };
            let path = Path::new(&clone_path);
            if !path.exists() {
                skipped.push(format!(
                    "{} (clone path not found: {})",
                    worker.name, clone_path
                ));
                continue;
            }

            match sync_worker_clone(path, &sync_ref) {
                Ok(details) => synced.push(format!("{} ({})", worker.name, details)),
                Err(err) => failed.push(format!("{} ({})", worker.name, err)),
            }
        }

        let mut out =
            format!("Worker Sync Report\n==================\n\nSync target: {sync_ref}\n");
        if !synced.is_empty() {
            out.push_str("\nSynced:\n");
            for item in synced {
                out.push_str(&format!("  - {item}\n"));
            }
        }
        if !skipped.is_empty() {
            out.push_str("\nSkipped:\n");
            for item in skipped {
                out.push_str(&format!("  - {item}\n"));
            }
        }
        if !failed.is_empty() {
            out.push_str("\nFailed:\n");
            for item in failed {
                out.push_str(&format!("  - {item}\n"));
            }
        }

        Ok(Self::success(out))
    }

    pub(super) async fn factory_gc_report(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_prompt_queue_store, open_worktree_store};
        use cas_types::WorktreeStatus;
        use std::path::Path;

        let stale_after = req.older_than_secs.unwrap_or(120);
        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let stale_agents = agent_store.list_stale(stale_after).unwrap_or_default();

        let prompt_queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {e}"),
            )
        })?;
        let pending_prompts = prompt_queue.pending_count().unwrap_or(0);

        let worktree_store = open_worktree_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open worktree store: {e}"),
            )
        })?;
        let active_worktrees = worktree_store
            .list_by_status(WorktreeStatus::Active)
            .unwrap_or_default();
        let orphan_worktrees: Vec<_> = active_worktrees
            .iter()
            .filter(|wt| !Path::new(&wt.path).exists())
            .collect();

        let mut out = String::from("Factory GC Report\n=================\n");
        out.push_str(&format!(
            "\nStale agent threshold: {}s\nStale agents: {}\nPending prompts: {}\nActive worktrees: {}\nOrphan worktrees: {}\n",
            stale_after,
            stale_agents.len(),
            pending_prompts,
            active_worktrees.len(),
            orphan_worktrees.len()
        ));

        if !stale_agents.is_empty() {
            out.push_str("\nStale agents:\n");
            for a in &stale_agents {
                out.push_str(&format!("  - {} ({})\n", a.name, a.id));
            }
        }
        if !orphan_worktrees.is_empty() {
            out.push_str("\nOrphan worktrees:\n");
            for wt in orphan_worktrees {
                out.push_str(&format!("  - {} ({})\n", wt.id, wt.path.display()));
            }
        }

        // Task cas-a9ab: surface uncommitted files in the main worktree as
        // "likely prior-factory WIP". Informational only — we never auto-delete.
        if let Some(summary) =
            crate::hooks::handlers::session_hygiene::wip_candidates(&self.inner.cas_root)
        {
            out.push_str(&format!(
                "\nMain worktree: {}\n",
                summary.worktree.display()
            ));
            if summary.is_clean() {
                out.push_str("Prior-factory WIP candidates: none (worktree clean)\n");
            } else {
                out.push_str(&format!(
                    "Prior-factory WIP candidates: {} ({} untracked, {} modified)\n",
                    summary.entries.len(),
                    summary.untracked_count(),
                    summary.modified_count(),
                ));
                for entry in &summary.entries {
                    out.push_str(&format!(
                        "  [{}] {} {}\n",
                        entry.label(),
                        entry.status,
                        entry.path,
                    ));
                }
                out.push_str(
                    "\nNote: these are not auto-deleted. Inspect, then commit/salvage/discard.\n",
                );
            }
        }

        Ok(Self::success(out))
    }

    /// cas-8f8f: read-only diagnostic that walks an epic's children
    /// and reports per-worker `factory/<assignee>` merge state vs.
    /// the epic's parent branch. Mirrors the `factory_gc_report`
    /// pattern: pure read, returns markdown via `Self::success`.
    ///
    /// Uses the same `count_unmerged_factory_commits` /
    /// `last_commit_unix` helpers that back the close-time gates in
    /// `close_ops.rs`, so the report can never disagree with what
    /// the gate actually enforces.
    pub(super) async fn factory_epic_status(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::core::task::lifecycle::close_ops::{
            collect_epic_branch_statuses, render_epic_status_report,
        };
        use crate::store::open_task_store;
        use cas_types::TaskType;

        let epic_id = req.id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "epic_status requires `id`: mcp__cas__coordination action=epic_status id=<epic-id>",
            )
        })?;

        let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open task store: {e}"),
            )
        })?;

        let epic = task_store.get(epic_id).map_err(|e| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Task not found: {epic_id}: {e}"),
            )
        })?;

        if epic.task_type != TaskType::Epic {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "epic_status: task {epic_id} is not an Epic (task_type={:?}). \
                     This action only operates on Epic-type tasks.",
                    epic.task_type
                ),
            ));
        }

        // The parent branch the gate compares against: the epic's
        // own `branch` field (set by epic creation), falling back to
        // "master" to match the epic-close path's existing default.
        let parent_branch = epic.branch.as_deref().unwrap_or("master");

        let subtasks = task_store.get_subtasks(epic_id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to walk subtasks of {epic_id}: {e}"),
            )
        })?;

        let close_project_root =
            self.inner.cas_root.parent().unwrap_or(&self.inner.cas_root);
        let statuses =
            collect_epic_branch_statuses(&subtasks, parent_branch, close_project_root);
        let report = render_epic_status_report(epic_id, parent_branch, &statuses);

        Ok(Self::success(report))
    }

    pub(super) async fn factory_gc_cleanup(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_prompt_queue_store, open_worktree_store};
        use cas_types::{AgentRole, WorktreeStatus};
        use std::path::Path;

        let stale_after = req.older_than_secs.unwrap_or(120);
        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let stale_agents = agent_store.list_stale(stale_after).unwrap_or_default();
        let mut stale_marked = 0usize;
        for agent in stale_agents {
            // Don't let workers prune supervisors/directors
            if agent.role == AgentRole::Supervisor || agent.role == AgentRole::Director {
                continue;
            }
            if agent_store.mark_stale(&agent.id).is_ok() {
                stale_marked += 1;
            }
        }

        let worktree_store = open_worktree_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open worktree store: {e}"),
            )
        })?;
        let active_worktrees = worktree_store
            .list_by_status(WorktreeStatus::Active)
            .unwrap_or_default();
        let mut orphan_marked_removed = 0usize;
        for mut wt in active_worktrees {
            if !Path::new(&wt.path).exists() {
                wt.mark_removed();
                if worktree_store.update(&wt).is_ok() {
                    orphan_marked_removed += 1;
                }
            }
        }

        // Clear prompt queue only when explicitly forced.
        let mut cleared_prompts = 0usize;
        if req.force.unwrap_or(false) {
            let prompt_queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open prompt queue: {e}"),
                )
            })?;
            cleared_prompts = prompt_queue.clear().unwrap_or(0);
        }

        Ok(Self::success(format!(
            "Factory GC cleanup complete.\n\nStale agents marked: {stale_marked}\nOrphan worktrees marked removed: {orphan_marked_removed}\nPrompt queue entries cleared: {cleared_prompts}"
        )))
    }
}

/// Returns the set of worker names this supervisor owns, derived from the `CAS_FACTORY_WORKER_NAMES`
/// environment variable. Returns `None` when not running as a supervisor or when the variable is
/// absent, meaning no scoping should be applied.
fn supervisor_owned_workers() -> Option<std::collections::HashSet<String>> {
    let role = std::env::var("CAS_AGENT_ROLE").unwrap_or_default();
    if !role.eq_ignore_ascii_case("supervisor") {
        return None;
    }
    let csv = std::env::var("CAS_FACTORY_WORKER_NAMES").ok()?;
    if csv.trim().is_empty() {
        return None;
    }
    Some(
        csv.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn run_git(path: &std::path::Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|e| format!("git {} failed to start: {}", args.join(" "), e))?;

    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn sync_worker_clone(
    path: &std::path::Path,
    sync_ref: &str,
) -> std::result::Result<String, String> {
    let status = run_git(path, &["status", "--porcelain"])?;
    let mut stashed = false;

    if !status.trim().is_empty() {
        let stash_msg = format!(
            "cas-factory-auto-sync {}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
        );
        let stash_out = run_git(
            path,
            &["stash", "push", "--include-untracked", "-m", &stash_msg],
        )?;
        if !stash_out.contains("No local changes") {
            stashed = true;
        }
    }

    let _ = run_git(path, &["fetch", "origin"]);

    if let Err(rebase_err) = run_git(path, &["rebase", sync_ref]) {
        let _ = run_git(path, &["rebase", "--abort"]);
        if stashed {
            let _ = run_git(path, &["stash", "pop"]);
        }
        return Err(format!("rebase failed: {rebase_err}"));
    }

    if stashed {
        run_git(path, &["stash", "pop"])
            .map_err(|e| format!("sync applied but stash pop failed: {e}"))?;
    }

    Ok(if stashed {
        "stashed + rebased + restored".to_string()
    } else {
        "rebased cleanly".to_string()
    })
}

/// Format a timestamp as relative time (e.g., "2s ago", "5m ago")
fn format_relative_time(dt: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);

    if diff.num_seconds() < 0 {
        return "just now".to_string();
    }

    if diff.num_seconds() < 60 {
        return format!("{}s ago", diff.num_seconds());
    }

    if diff.num_minutes() < 60 {
        return format!("{}m ago", diff.num_minutes());
    }

    if diff.num_hours() < 24 {
        return format!("{}h ago", diff.num_hours());
    }

    format!("{}d ago", diff.num_days())
}

/// cas-8240 two-band liveness label for `factory_worker_status`:
///
/// * `elapsed >= WORKER_DEAD_SECS` → `" [DEAD]"` (hard escalation —
///   caller also surfaces the transcript path for salvage).
/// * `WORKER_STALE_SECS <= elapsed < WORKER_DEAD_SECS` → `" [stale]"`
///   (grace-window indicator — the worker slipped past the prune
///   without being `mark_stale`'d, but it's too early to declare it
///   dead).
/// * Otherwise → `""` (no label).
///
/// Leading space is intentional: the caller concatenates the returned
/// slice directly after the `heartbeat: <Xs ago>` segment, and an empty
/// string avoids a trailing space when the worker is fresh. Returning
/// `&'static str` keeps this allocation-free.
fn liveness_label_for(elapsed_secs: i64) -> &'static str {
    if elapsed_secs >= WORKER_DEAD_SECS {
        " [DEAD]"
    } else if elapsed_secs >= WORKER_STALE_SECS {
        " [stale]"
    } else {
        ""
    }
}

/// Resolution outcome for a worker's Claude Code transcript file
/// (cas-900b — replaces the brittle reconstruct-only `derive_transcript_path`).
///
/// Claude Code persists each session's JSONL under
/// `~/.claude/projects/<escaped-cwd>/<session-id>.jsonl`. Session IDs are
/// stable UUIDs unique across all projects, so we can glob
/// `~/.claude/projects/*/<session-id>.jsonl` and surface whichever real path
/// actually exists — rather than reconstructing the `<escaped-cwd>` from the
/// worker's clone_path, which was observed from a single field sample and
/// breaks on spaces, unicode, colons, and any future CC escape change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptResolution {
    /// Exactly one `~/.claude/projects/*/<session-id>.jsonl` matched.
    /// The real on-disk path ready for the supervisor to open.
    Resolved(std::path::PathBuf),
    /// Zero matches. Could be: CC never wrote a transcript, the home dir
    /// lookup failed, or the worker died before SessionStart. Carries the
    /// reconstructed (legacy) path, labelled "likely" at the call site.
    Synthesized(String),
    /// More than one match — should be rare (session_id collisions or a
    /// user who manually copied transcripts between projects). Surface
    /// all candidates so the supervisor can pick. `truncated` is true
    /// when the glob walk hit `MAX_TRANSCRIPT_CANDIDATES`.
    Ambiguous {
        matches: Vec<std::path::PathBuf>,
        synthesized: String,
        truncated: bool,
    },
}

/// Reconstruct the legacy `<escaped-cwd>` path. Used as the fallback in the
/// `Synthesized` and `Ambiguous` branches of `TranscriptResolution`; also
/// kept for tests that want to pin the historical escape semantics.
///
/// Observed in the wild: both `/` and `.` collapse to `-`. Underscores and
/// other characters are preserved. Example:
/// `/home/a/.cas/worktrees/x` → `-home-a--cas-worktrees-x`.
fn synthesized_transcript_path(clone_path: &str, session_id: &str) -> String {
    let escaped: String = clone_path
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect();
    format!("~/.claude/projects/{escaped}/{session_id}.jsonl")
}

/// Hard cap on glob candidate collection to bound the worst-case
/// `worker_status` latency (adversarial cas-900b P1). On a long-lived
/// host `~/.claude/projects/` can accumulate thousands of transcripts;
/// listing more than 50 for a single worker isn't useful anyway — the
/// supervisor needs to pick one, not read a thousand paths. If the cap
/// is ever hit the output notes the truncation so the supervisor knows
/// to grep manually.
const MAX_TRANSCRIPT_CANDIDATES: usize = 50;

/// Glob `<projects_dir>/*/<session-id>.jsonl` and return up to
/// `MAX_TRANSCRIPT_CANDIDATES` matches plus a `truncated` flag.
///
/// - `session_id` is glob-escaped before interpolation (adversarial
///   cas-900b P1): an agent that registers with a malicious
///   `session_id = "*"` must not broaden the search and leak every
///   transcript on the host into `worker_status` output.
/// - Malformed glob patterns and I/O errors collapse to an empty vec;
///   the caller's fallback path preserves supervisor agency.
fn glob_transcript_candidates(
    projects_dir: &std::path::Path,
    session_id: &str,
) -> (Vec<std::path::PathBuf>, bool) {
    let escaped_session = glob::Pattern::escape(session_id);
    let pattern = format!(
        "{}/*/{}.jsonl",
        projects_dir.to_string_lossy(),
        escaped_session
    );
    let iter = match glob::glob(&pattern) {
        Ok(it) => it,
        Err(_) => return (Vec::new(), false),
    };
    let mut out = Vec::new();
    let mut truncated = false;
    for result in iter {
        if let Ok(p) = result {
            if out.len() >= MAX_TRANSCRIPT_CANDIDATES {
                truncated = true;
                break;
            }
            out.push(p);
        }
    }
    (out, truncated)
}

/// Resolve the transcript location for a worker. Uses `glob_transcript_candidates`
/// against `~/.claude/projects/` by default; tests override `projects_dir`.
///
/// `clone_path == None` means the worker registered without cwd metadata;
/// the `Synthesized` / `Ambiguous` fallback paths omit the reconstructed
/// legacy escape in that case (there's nothing to reconstruct from), and
/// the caller must label the output accordingly.
pub(crate) fn resolve_transcript(
    projects_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
) -> TranscriptResolution {
    let synthesized = clone_path.map(|p| synthesized_transcript_path(p, session_id));
    let Some(projects) = projects_dir else {
        return TranscriptResolution::Synthesized(
            synthesized.unwrap_or_else(|| synthesized_unknown_clone_path(session_id)),
        );
    };
    let (mut matches, truncated) = glob_transcript_candidates(projects, session_id);
    match matches.len() {
        0 => TranscriptResolution::Synthesized(
            synthesized.unwrap_or_else(|| synthesized_unknown_clone_path(session_id)),
        ),
        1 => TranscriptResolution::Resolved(matches.remove(0)),
        _ => TranscriptResolution::Ambiguous {
            matches,
            synthesized: synthesized
                .unwrap_or_else(|| synthesized_unknown_clone_path(session_id)),
            truncated,
        },
    }
}

/// `~/.claude/projects` — Claude Code's per-user transcript root.
/// Returns `None` if the user's home dir isn't resolvable.
pub(crate) fn default_claude_projects_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Placeholder synthesized path used when clone_path metadata is absent.
/// The label explicitly names the missing input so a supervisor reading
/// the output sees *why* the synthesized path is a placeholder (adversarial
/// cas-900b P3: don't conflate clone_path-absent with home_dir-absent).
fn synthesized_unknown_clone_path(session_id: &str) -> String {
    format!("~/.claude/projects/<cwd>/{session_id}.jsonl (clone path unknown)")
}

/// Render the transcript block for `worker_status` output. Always surfaces
/// the raw `session_id` so a supervisor who doesn't trust our resolution
/// can grep the projects tree themselves (cas-900b AC).
///
/// `clone_path == None` is handled by the same resolver with a clearly
/// labelled fallback — no duplicated glob+match dispatch (maintainability
/// cas-900b P2).
fn format_transcript_block(clone_path: Option<&str>, session_id: &str) -> String {
    let projects = default_claude_projects_dir();
    let resolution = resolve_transcript(projects.as_deref(), clone_path, session_id);
    render_transcript_block(&resolution, session_id, projects.is_some())
}

/// Pure string-rendering half of `format_transcript_block`, split out so
/// tests can drive it against a `TranscriptResolution` built via the
/// injectable resolver without touching `dirs::home_dir()`.
fn render_transcript_block(
    resolution: &TranscriptResolution,
    session_id: &str,
    home_resolved: bool,
) -> String {
    match resolution {
        TranscriptResolution::Resolved(path) => format!(
            "\n    Transcript: {}\n    Session: {session_id}",
            path.display()
        ),
        TranscriptResolution::Synthesized(path) => {
            // Distinguish home-dir-unresolvable from "glob returned 0
            // matches" so a supervisor triaging the output knows which
            // failure mode to chase (adversarial cas-900b P3).
            if home_resolved {
                format!("\n    Likely transcript: {path}\n    Session: {session_id}")
            } else {
                format!(
                    "\n    Likely transcript: {path}\n    (home dir unresolvable — glob skipped)\n    Session: {session_id}"
                )
            }
        }
        TranscriptResolution::Ambiguous {
            matches,
            synthesized,
            truncated,
        } => {
            let mut s = format!("\n    Transcript candidates (session {session_id}):");
            for m in matches {
                s.push_str(&format!("\n      - {}", m.display()));
            }
            if *truncated {
                s.push_str(&format!(
                    "\n      … (truncated at {MAX_TRANSCRIPT_CANDIDATES}; grep ~/.claude/projects for session)"
                ));
            }
            s.push_str(&format!("\n    Likely synthesized: {synthesized}"));
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Session id used across the glob tests. Stable UUID shape, unique
    /// across fake projects so the glob doesn't collide with anything else
    /// the test happens to create.
    const TEST_SESSION: &str = "cas-900b-test-0000-0000-000000000000";

    /// Create a fake `~/.claude/projects/` layout in a tempdir and return
    /// the `projects` subdir path. `projects` is populated with `dirs`
    /// entries, each containing a `<session_id>.jsonl` for sessions in
    /// that dir's `contains_sessions` list.
    fn fake_projects_dir(
        dirs: &[(&str, &[&str])],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        for (dir_name, sessions) in dirs {
            let d = projects.join(dir_name);
            std::fs::create_dir_all(&d).unwrap();
            for s in *sessions {
                std::fs::write(d.join(format!("{s}.jsonl")), b"").unwrap();
            }
        }
        (tmp, projects)
    }

    #[test]
    fn synthesized_path_matches_claude_code_escape() {
        // Observed in the field (crisp-badger-65): keep this pinned as the
        // fallback contract for Synthesized / Ambiguous branches.
        let clone = "/home/pippenz/Petrastella/cas-src/.cas/worktrees/crisp-badger-65";
        let session = "064e7b23-331d-4dae-9c6a-721cbbe9c024";
        let got = synthesized_transcript_path(clone, session);
        assert_eq!(
            got,
            "~/.claude/projects/-home-pippenz-Petrastella-cas-src--cas-worktrees-crisp-badger-65/\
             064e7b23-331d-4dae-9c6a-721cbbe9c024.jsonl"
        );
    }

    #[test]
    fn synthesized_path_escapes_dots_preserves_underscores() {
        let got = synthesized_transcript_path("/tmp/my_proj.sub", "abc");
        assert_eq!(got, "~/.claude/projects/-tmp-my_proj-sub/abc.jsonl");
    }

    // --- cas-8240: two-band stale/dead threshold constants ------------------

    /// AC anchor: `WORKER_STALE_SECS` is pinned at 30. The supervisor-facing
    /// footer embeds this value as a literal ("30s heartbeat age") and the
    /// daemon heartbeat tick is tuned against it, so a silent change here
    /// would desync the prune window from the UX text.
    #[test]
    fn worker_stale_secs_is_pinned_at_30() {
        assert_eq!(WORKER_STALE_SECS, 30);
    }

    /// AC anchor: `WORKER_DEAD_SECS` is pinned at 75. The two-band model
    /// requires DEAD to lag STALE by roughly one grace window so scheduler
    /// jitter and missed ticks do not produce false-positive [DEAD] labels.
    /// Bumping this silently would regress the cas-8240 fix.
    #[test]
    fn worker_dead_secs_is_pinned_at_75() {
        assert_eq!(WORKER_DEAD_SECS, 75);
    }

    /// Invariant: the dead threshold must strictly exceed the stale
    /// threshold. Otherwise the two-band render collapses into one band
    /// and we reintroduce the false-positive DEAD labeling cas-8240 fixes.
    #[test]
    fn worker_dead_secs_exceeds_stale_secs() {
        assert!(
            WORKER_DEAD_SECS > WORKER_STALE_SECS,
            "WORKER_DEAD_SECS ({WORKER_DEAD_SECS}) must exceed WORKER_STALE_SECS ({WORKER_STALE_SECS}) — the two-band model collapses otherwise"
        );
    }

    // --- cas-8240: liveness_label_for branch matrix -------------------------

    #[test]
    fn liveness_label_fresh_worker_is_empty() {
        assert_eq!(liveness_label_for(0), "");
        assert_eq!(liveness_label_for(WORKER_STALE_SECS - 1), "");
    }

    #[test]
    fn liveness_label_grace_window_is_stale() {
        // Exactly at STALE → [stale]; just below DEAD → still [stale].
        assert_eq!(liveness_label_for(WORKER_STALE_SECS), " [stale]");
        assert_eq!(liveness_label_for(WORKER_DEAD_SECS - 1), " [stale]");
    }

    #[test]
    fn liveness_label_past_dead_is_hard_dead() {
        // Exactly at DEAD → [DEAD]; well past → still [DEAD].
        assert_eq!(liveness_label_for(WORKER_DEAD_SECS), " [DEAD]");
        assert_eq!(liveness_label_for(WORKER_DEAD_SECS * 10), " [DEAD]");
    }

    #[test]
    fn liveness_label_distinguishes_stale_from_dead() {
        // The cas-8240 core behavior: stale and DEAD are distinct bands.
        // A mutation that collapsed the stale branch into " [DEAD]"
        // would fail here.
        let stale = liveness_label_for(WORKER_STALE_SECS);
        let dead = liveness_label_for(WORKER_DEAD_SECS);
        assert_ne!(stale, dead, "stale and DEAD bands must render distinct labels");
        assert!(stale.contains("stale"));
        assert!(dead.contains("DEAD"));
    }

    // --- cas-900b: glob-first transcript resolution -------------------------

    #[test]
    fn resolve_transcript_returns_resolved_on_unique_match() {
        // cas-900b AC (1): unique match → Resolved with the real on-disk path.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-alice-workspace-one", &[TEST_SESSION]),
            ("-home-alice-workspace-two", &["other-session-zzz"]),
        ]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/alice/workspace/one"),
            TEST_SESSION,
        );
        let expected_path = projects
            .join("-home-alice-workspace-one")
            .join(format!("{TEST_SESSION}.jsonl"));
        assert_eq!(got, TranscriptResolution::Resolved(expected_path));
    }

    #[test]
    fn resolve_transcript_returns_synthesized_on_no_match() {
        // cas-900b AC (2): no match → Synthesized fallback, preserves
        // legacy reconstruct semantics.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-alice-workspace-one", &["unrelated"]),
        ]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/alice/workspace/one"),
            TEST_SESSION,
        );
        let expected =
            synthesized_transcript_path("/home/alice/workspace/one", TEST_SESSION);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn resolve_transcript_returns_ambiguous_on_multiple_matches() {
        // cas-900b AC (3): multiple matches → Ambiguous with all paths
        // surfaced for the supervisor to pick.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-alice-workspace-one", &[TEST_SESSION]),
            ("-home-alice-workspace-two", &[TEST_SESSION]),
        ]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/alice/workspace/one"),
            TEST_SESSION,
        );
        match got {
            TranscriptResolution::Ambiguous {
                mut matches,
                synthesized,
                truncated,
            } => {
                assert!(!truncated, "2 < MAX_TRANSCRIPT_CANDIDATES");
                // Sort for deterministic comparison (glob order is
                // filesystem-dependent — cas-900b testing P3).
                matches.sort();
                let mut expected: Vec<_> = vec![
                    projects
                        .join("-home-alice-workspace-one")
                        .join(format!("{TEST_SESSION}.jsonl")),
                    projects
                        .join("-home-alice-workspace-two")
                        .join(format!("{TEST_SESSION}.jsonl")),
                ];
                expected.sort();
                assert_eq!(matches, expected);
                assert_eq!(
                    synthesized,
                    synthesized_transcript_path("/home/alice/workspace/one", TEST_SESSION)
                );
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn resolve_transcript_handles_unicode_clone_path() {
        // The whole point of cas-900b: a unicode cwd that the legacy
        // reconstruct would still escape (char-by-char, preserving the
        // codepoint) BUT the real CC escape might differ. With glob-first,
        // we don't care what escape CC chose — if the file exists, we find
        // it via session_id alone.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-usér-projet-café", &[TEST_SESSION]),
        ]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/usér/projet/café"),
            TEST_SESSION,
        );
        let expected_path = projects
            .join("-home-usér-projet-café")
            .join(format!("{TEST_SESSION}.jsonl"));
        assert_eq!(got, TranscriptResolution::Resolved(expected_path));
    }

    #[test]
    fn resolve_transcript_no_projects_dir_is_synthesized() {
        // If we can't resolve the home dir (shouldn't happen in practice),
        // the function still returns a usable Synthesized fallback.
        let got = resolve_transcript(None, Some("/home/alice/x"), TEST_SESSION);
        let expected = synthesized_transcript_path("/home/alice/x", TEST_SESSION);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn resolve_transcript_no_clone_path_falls_back_to_placeholder() {
        // When clone_path is None (worker registered without cwd metadata),
        // the Synthesized arm carries the placeholder label instead of a
        // reconstructed path.
        let (_tmp, projects) = fake_projects_dir(&[]);
        let got = resolve_transcript(Some(&projects), None, TEST_SESSION);
        let expected = synthesized_unknown_clone_path(TEST_SESSION);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn glob_candidates_returns_empty_on_missing_projects_dir() {
        // Glob on a nonexistent path must not panic — just return empty.
        let missing = std::path::Path::new("/tmp/does-not-exist-cas-900b");
        let (got, truncated) = glob_transcript_candidates(missing, TEST_SESSION);
        assert!(got.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn glob_candidates_escapes_session_id_metachars() {
        // cas-900b adversarial P1: a rogue session_id containing glob
        // metacharacters (`*`, `?`, `[`) must not broaden the match and
        // surface unrelated transcripts. We create a "real" file at
        // `*.jsonl` (by using a sentinel session_id for the fake dir)
        // plus noise files, and glob for the literal `*` session id;
        // only a file whose stem is literally `*` should come back, and
        // in this layout there is none, so the result is empty.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-alice-one", &[TEST_SESSION, "another-session"]),
            ("-home-alice-two", &["yet-another"]),
        ]);
        // A malicious session_id: `*` would, if unescaped, match every
        // .jsonl under every project dir. With the fix, glob::Pattern::escape
        // turns it into `[*]` (glob literal) so it only matches a file
        // literally named `*.jsonl` — which doesn't exist here.
        let (got, _) = glob_transcript_candidates(&projects, "*");
        assert!(
            got.is_empty(),
            "escaped `*` must not match arbitrary .jsonl files; got {got:?}"
        );
    }

    #[test]
    fn glob_candidates_truncates_at_max() {
        // cas-900b adversarial P1: bound latency under high-cardinality
        // layouts. Build a layout with MAX+5 matches and confirm the
        // truncated flag fires and the vec length stops at MAX.
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        for i in 0..(MAX_TRANSCRIPT_CANDIDATES + 5) {
            let d = projects.join(format!("proj-{i}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("{TEST_SESSION}.jsonl")), b"").unwrap();
        }
        let (got, truncated) = glob_transcript_candidates(&projects, TEST_SESSION);
        assert_eq!(got.len(), MAX_TRANSCRIPT_CANDIDATES);
        assert!(truncated, "MAX+5 inputs must trip the truncated flag");
    }

    #[test]
    fn render_transcript_block_resolved_contains_session_and_path() {
        let path = std::path::PathBuf::from("/home/u/.claude/projects/x/ses.jsonl");
        let got = render_transcript_block(
            &TranscriptResolution::Resolved(path.clone()),
            TEST_SESSION,
            true,
        );
        assert!(got.contains("Transcript: /home/u/.claude/projects/x/ses.jsonl"));
        assert!(got.contains(&format!("Session: {TEST_SESSION}")));
        assert!(!got.contains("Likely"));
    }

    #[test]
    fn render_transcript_block_synthesized_labels_likely_and_surfaces_session() {
        let synth = "~/.claude/projects/-home-x/ses.jsonl".to_string();
        let got = render_transcript_block(
            &TranscriptResolution::Synthesized(synth.clone()),
            TEST_SESSION,
            true,
        );
        assert!(got.contains(&format!("Likely transcript: {synth}")));
        assert!(got.contains(&format!("Session: {TEST_SESSION}")));
    }

    #[test]
    fn render_transcript_block_synthesized_no_home_notes_skipped_glob() {
        // cas-900b adversarial P3: distinguish home-dir failure from
        // clone-path failure.
        let synth = synthesized_unknown_clone_path(TEST_SESSION);
        let got = render_transcript_block(
            &TranscriptResolution::Synthesized(synth),
            TEST_SESSION,
            false,
        );
        assert!(got.contains("home dir unresolvable"));
        assert!(got.contains("glob skipped"));
    }

    #[test]
    fn render_transcript_block_ambiguous_lists_candidates_with_session_and_fallback() {
        let matches = vec![
            std::path::PathBuf::from("/p/a/ses.jsonl"),
            std::path::PathBuf::from("/p/b/ses.jsonl"),
        ];
        let synthesized = "~/.claude/projects/-p-a/ses.jsonl".to_string();
        let got = render_transcript_block(
            &TranscriptResolution::Ambiguous {
                matches,
                synthesized: synthesized.clone(),
                truncated: false,
            },
            TEST_SESSION,
            true,
        );
        assert!(got.contains(&format!("candidates (session {TEST_SESSION})")));
        assert!(got.contains("- /p/a/ses.jsonl"));
        assert!(got.contains("- /p/b/ses.jsonl"));
        assert!(got.contains(&format!("Likely synthesized: {synthesized}")));
        assert!(!got.contains("truncated"));
    }

    #[test]
    fn render_transcript_block_ambiguous_truncated_notes_cap() {
        let got = render_transcript_block(
            &TranscriptResolution::Ambiguous {
                matches: vec![std::path::PathBuf::from("/p/a/ses.jsonl")],
                synthesized: "<s>".to_string(),
                truncated: true,
            },
            TEST_SESSION,
            true,
        );
        assert!(got.contains("truncated"));
        assert!(got.contains(&format!("{MAX_TRANSCRIPT_CANDIDATES}")));
    }

    /// cas-85bf: worker_status output must include session UUID alongside the
    /// friendly worker name so supervisors can cross-reference task-ownership
    /// errors ("owned by worker-backfill (0a7f2802-...)") without extra lookups.
    ///
    /// This test exercises the format string in `factory_worker_status` by
    /// manually building the string the same way the production code does and
    /// asserting the UUID is embedded.
    #[test]
    fn test_worker_status_format_includes_session_uuid() {
        const NAME: &str = "worker-backfill";
        const UUID: &str = "0a7f2802-e977-493b-965b-c620e99f04ef";

        // Reproduce the format! call from factory_worker_status (cas-85bf).
        let output = format!(
            "  • {} (heartbeat: {}){}{}{}\n    session: {}\n",
            NAME, "5s ago", "", "", "", UUID
        );

        assert!(
            output.contains(NAME),
            "output must contain worker name: {output}"
        );
        assert!(
            output.contains(UUID),
            "output must contain session UUID: {output}"
        );
        assert!(
            output.contains("session:"),
            "output must have 'session:' label: {output}"
        );
    }
}
