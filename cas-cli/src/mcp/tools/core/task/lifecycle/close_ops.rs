use crate::harness_policy::{
    is_supervisor_from_env, is_worker_without_subagents_from_env, supervisor_harness_from_env,
    verification_policy, worker_harness_from_env,
};
use crate::mcp::tools::core::imports::*;

/// Maximum time a task may sit in `pending_verification` before the close path
/// treats the task-verifier subagent as dead, auto-escalates, and releases the
/// jail. Addresses cas-c29a (within-task verification deadlock): if the
/// verifier subagent crashes, is never spawned, or fails silently, the
/// original dispatch-request row stays in `Error` status forever and every
/// close retry returns `VERIFICATION REQUIRED`.
const VERIFICATION_JAIL_TIMEOUT_SECS: i64 = 600;

/// Heartbeat staleness threshold (seconds) for deciding whether an assignee
/// is still considered active for verification-skip purposes. Aligned with
/// the same 5-minute window used by task-claim reclaim.
const ASSIGNEE_STALE_SECS: i64 = 300;

/// Marker prefix used on the dispatch-request verification row (see
/// lines ~255-272 below). Used to distinguish a stale dispatch from a real
/// verifier-written Error verdict during auto-escalation.
const DISPATCH_SUMMARY_PREFIX: &str = "Dispatch requested";

/// Why the close path decided to skip (or not skip) the task-verifier step
/// for a given close attempt.
///
/// Carried through to the response message so the audit trail cites the
/// real reason instead of the catch-all "assignee inactive" phrase that
/// surfaced cas-3bd4.
///
/// The pre-cas-3bd4 implementation represented this as a single
/// `assignee_inactive: bool`. Every lookup failure — including the
/// very-common name-vs-id mismatch described below — defaulted to `true`
/// and the success message confidently lied that the assignee was inactive.
/// This enum preserves the same skip *behavior* (supervisor still closes
/// orphaned or genuinely-stale tasks without a verifier hop) but forces
/// every skip reason to be named.
///
/// ## Why the old `agent_store.get(task.assignee)` kept returning "inactive"
///
/// `task.assignee` is set by `task_claiming.rs:89` to
/// `Some(agent_name.clone())` — the human-readable display name, e.g.
/// `"mighty-viper-52"`. But `AgentStore::get(id)` runs `WHERE id = ?` in
/// `ops_agent.rs:79`, and `id` is the session-id (a UUID-like
/// identifier), not the name. The lookup never found the row, so
/// `unwrap_or(true)` treated the worker as inactive even though it was
/// actively holding a fresh lease. `compute_verification_skip_reason`
/// fixes this by consulting the task's active lease first — `TaskLease`
/// stores the real `agent_id`, not the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VerificationSkipReason {
    /// The assignee is alive and no bypass flag was set. Verification
    /// runs normally; this is *not* a skip.
    None,
    /// The task has no assignee at all. Treated as orphaned; legacy
    /// callers reached this via the same skip path.
    NoAssignee,
    /// The assignee exists and is registered, but their heartbeat or
    /// lease is stale. `minutes_stale` is the observed staleness if we
    /// could measure it.
    AssigneeInactive { minutes_stale: Option<i64> },
    /// `task.assignee` is set but we cannot resolve it through the
    /// lease *or* a direct id lookup. The agent may have been GC'd or
    /// the assignee row holds an old display name no longer in the
    /// agent store. Skip verification and cite the real reason.
    AssigneeUnknown,
    /// Supervisor is closing a task whose assignee is still alive and
    /// has explicitly requested a verification skip via
    /// `bypass_code_review=true`. Separate from `AssigneeInactive` so
    /// the audit note reflects supervisor intent, not worker state.
    SupervisorBypass,
}

impl VerificationSkipReason {
    /// Whether this reason short-circuits the verification gate.
    pub(crate) fn is_skip(&self) -> bool {
        !matches!(self, VerificationSkipReason::None)
    }

    /// Short human-readable suffix appended to the `Closed task:` line.
    /// Must start with a leading space so it slots cleanly into the
    /// format string.
    pub(crate) fn response_suffix(&self, verification_enabled: bool) -> String {
        match self {
            VerificationSkipReason::None => {
                if verification_enabled {
                    " (verified)".to_string()
                } else {
                    String::new()
                }
            }
            VerificationSkipReason::NoAssignee => {
                " (verification skipped — orphaned task, no assignee)".to_string()
            }
            VerificationSkipReason::AssigneeInactive {
                minutes_stale: Some(m),
            } => {
                format!(" (verification skipped — assignee inactive for {m}m)")
            }
            VerificationSkipReason::AssigneeInactive { minutes_stale: None } => {
                " (verification skipped — assignee lease expired)".to_string()
            }
            VerificationSkipReason::AssigneeUnknown => {
                " (verification skipped — assignee unknown)".to_string()
            }
            VerificationSkipReason::SupervisorBypass => {
                " (verification skipped — supervisor bypass via bypass_code_review=true)"
                    .to_string()
            }
        }
    }

    /// Reason text written to the `Skipped` verification row so the
    /// audit trail records the accurate reason alongside the row, not
    /// just in the response text.
    pub(crate) fn audit_reason(&self) -> String {
        match self {
            VerificationSkipReason::None => String::new(),
            VerificationSkipReason::NoAssignee => {
                "Closed via supervisor bypass — task had no assignee (orphaned).".to_string()
            }
            VerificationSkipReason::AssigneeInactive {
                minutes_stale: Some(m),
            } => format!(
                "Closed via supervisor bypass — assignee inactive for {m} minute(s) at close time."
            ),
            VerificationSkipReason::AssigneeInactive { minutes_stale: None } => {
                "Closed via supervisor bypass — assignee lease had expired at close time."
                    .to_string()
            }
            VerificationSkipReason::AssigneeUnknown => {
                "Closed via supervisor bypass — assignee row not found in agent store (likely \
                 a stale or renamed agent)."
                    .to_string()
            }
            VerificationSkipReason::SupervisorBypass => {
                "Closed via supervisor bypass — bypass_code_review=true explicitly set by \
                 supervisor while assignee was still active."
                    .to_string()
            }
        }
    }
}

impl CasCore {
    pub async fn cas_task_close(
        &self,
        Parameters(req): Parameters<TaskCloseRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        // For Epics: Check that all worker branches are merged before verification
        // This ensures epic-level verification runs on the complete merged code
        if task.task_type == TaskType::Epic {
            let target_branch = task.branch.as_deref().unwrap_or("master");
            let unmerged = check_unmerged_epic_branches(&req.id, target_branch);
            if !unmerged.is_empty() {
                let branch_list = unmerged.join("\n  - ");
                return Ok(Self::tool_error(format!(
                    "⚠️ MERGE REQUIRED\n\n\
                    Epic {} has {} unmerged worker branch(es):\n  - {}\n\n\
                    Worker branches must be merged to {} before closing the epic.\n\n\
                    Use /factory-merge-epic to:\n\
                    1. Fetch all worker branches from remote\n\
                    2. Merge each branch to {}\n\
                    3. Run tests on the merged code\n\n\
                    After merging, call mcp__cas__task action=close id={} again.",
                    req.id,
                    unmerged.len(),
                    branch_list,
                    target_branch,
                    target_branch,
                    req.id
                )));
            }

            // cas-8f8f: per-child factory-branch merge-state guard for
            // epic close. The check above (`check_unmerged_epic_branches`)
            // operates on the epic's own branch namespace; this gate
            // walks every child task's `factory/<assignee>` branch and
            // rejects when any has stranded commits relative to the
            // epic branch. Bypass-immune (data-state guard, not a
            // review gate). Diagnostic surface for in-flight queries
            // is `mcp__cas__coordination action=epic_status id=<epic>`.
            //
            // Errors from `get_subtasks` MUST surface as a hard error,
            // never a silent empty-list pass. Round-1 cas-code-review
            // (correctness P1) caught the `unwrap_or_default()` failure
            // mode: a transient SQLite error would map to "no children"
            // and the gate would Proceed — defeating the entire
            // enforcement that this task adds. Mirror the conservative
            // pattern at line ~869 (`epic_subtask_receipts_cover`)
            // where a store error is treated as gate-blocking.
            let subtasks = task_store.get_subtasks(&req.id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "epic-close merge gate: failed to read subtasks of {epic_id}: {e}",
                    epic_id = req.id
                )),
                data: None,
            })?;
            let close_project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
            match run_epic_close_merge_gate(
                &task,
                &req,
                target_branch,
                close_project_root,
                &subtasks,
            ) {
                EpicCloseGateOutcome::Proceed => {}
                EpicCloseGateOutcome::Reject(msg) => {
                    return Ok(Self::tool_error(msg));
                }
            }
        }

        // cas-95ce: per-task close-time merge-state guard. Mirrors the
        // shape of the epic check above, but at the worker scope: when
        // a non-epic task with an assignee is being closed, reject if
        // `factory/<assignee>` carries commits that haven't landed on
        // the parent epic branch. Runs BEFORE the verification policy
        // and the cas-code-review bypass — `bypass_code_review=true`
        // cannot skip this guard because it is a data-state check, not
        // a review gate. See `run_factory_branch_merge_gate` for the
        // full skip matrix and EPIC cas-754b for context.
        if task.task_type != TaskType::Epic && task.assignee.is_some() {
            // Resolve the parent epic's branch via the existing
            // ParentChild dependency. If no parent epic is recorded,
            // fall back to "main" — the modern default-branch name.
            // (The epic-close path at line ~162 still defaults to
            // "master" because that field predates the convention
            // change. We deliberately do not align them here:
            // worker tasks created in this codepath are universally
            // attached to an epic with an explicit `branch` field,
            // so the fallback is a defense-in-depth string for an
            // already-pathological case. If that fallback string
            // does not resolve locally, `git merge-base` fails and
            // count_unmerged_factory_commits returns 0 — i.e., the
            // gate degrades to Proceed, never to a false Reject.
            // See cas-95ce notes for the rationale.)
            let parent_branch = task_store
                .get_parent_epic(&req.id)
                .ok()
                .flatten()
                .and_then(|p| p.branch)
                .unwrap_or_else(|| "main".to_string());
            let close_project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
            match run_factory_branch_merge_gate(
                &task,
                &req,
                &parent_branch,
                close_project_root,
            ) {
                MergeStateGateOutcome::Proceed => {}
                MergeStateGateOutcome::Reject(msg) => {
                    return Ok(Self::tool_error(msg));
                }
            }
        }

        // Check verification status if enabled
        let config = self.load_config();
        let policy = verification_policy(supervisor_harness_from_env(), worker_harness_from_env());
        let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
            .map(|r| r.eq_ignore_ascii_case("worker"))
            .unwrap_or(false)
            && std::env::var("CAS_FACTORY_MODE").is_ok();
        let verification_enabled = config.verification_enabled()
            && if task.task_type == TaskType::Epic {
                if is_supervisor_from_env() {
                    policy.epic_required()
                } else {
                    true
                }
            } else {
                policy.task_required()
            };

        // Skip verification for orphaned tasks: if caller is supervisor and the
        // task's assignee is inactive (heartbeat expired or lease gone), allow
        // close without verification. cas-3bd4: compute the reason as a typed
        // enum so the response message cites the actual state instead of
        // defaulting to "assignee inactive" for every lookup failure.
        let skip_reason = if verification_enabled && is_supervisor_from_env() {
            self.compute_verification_skip_reason(&task, &req)
        } else {
            VerificationSkipReason::None
        };
        let skip_verification = skip_reason.is_skip();

        // Also allow supervisor to skip verification jail when they are the
        // task assignee for a non-epic task (fixes supervisor self-close deadlock).
        let supervisor_is_assignee = is_supervisor_from_env()
            && task.task_type != TaskType::Epic
            && self
                .get_agent_id()
                .ok()
                .map(|aid| task.assignee.as_deref() == Some(aid.as_str()))
                .unwrap_or(false);

        if verification_enabled && !skip_verification {
            let is_worker_without_subagents = is_worker_without_subagents_from_env();

            // Check for approved verification
            if let Ok(verification_store) = self.open_verification_store() {
                // Determine verification type and agent based on task type
                let is_epic = task.task_type == TaskType::Epic;
                let (verification_type, verifier_agent) = if is_epic {
                    (VerificationType::Epic, "task-verifier")
                } else {
                    (VerificationType::Task, "task-verifier")
                };

                // Get the appropriate verification (by type for epics, any for tasks)
                let latest = if is_epic {
                    verification_store.get_latest_for_task_by_type(&req.id, verification_type)
                } else {
                    verification_store.get_latest_for_task(&req.id)
                };

                // Whether a prior verification row (of any status) already
                // exists. Used below to decide whether to persist a fresh
                // dispatch-request marker so the close attempt is durably
                // observable instead of fire-and-forget.
                let had_prior_verification = matches!(&latest, Ok(Some(_)));

                match latest {
                    Ok(Some(v))
                        if v.status == VerificationStatus::Approved
                            || v.status == VerificationStatus::Skipped =>
                    {
                        // Verification approved or explicitly skipped
                        // (supervisor bypass row from a prior orphaned close) —
                        // proceed with close. See cas-82d6.
                    }
                    Ok(Some(v)) if v.status == VerificationStatus::Rejected => {
                        // Verification rejected, block close
                        // Only auto-claim if the closing agent is the task's assignee.
                        // If a supervisor closes a worker's task, skip the lease to avoid
                        // locking the task to the supervisor.
                        let is_assignee = self
                            .get_agent_id()
                            .ok()
                            .map(|aid| task.assignee.as_deref() == Some(aid.as_str()))
                            .unwrap_or(false);
                        if is_assignee {
                            self.auto_claim_for_verification(&req.id, task_store.as_ref())?;
                        }

                        let issue_count = v.issues.len();
                        let blocking = v
                            .issues
                            .iter()
                            .filter(|i| i.severity == crate::types::IssueSeverity::Blocking)
                            .count();

                        // Include new close reason if provided (may have been fixed)
                        let close_reason_note = if let Some(ref reason) = req.reason {
                            format!(
                                "\n\n## New Close Reason Provided\n\
                                ```\n{reason}\n```\n\n\
                                If resubmitting, ensure the close reason describes COMPLETED work only.\n\
                                Do not use language like 'remaining', 'beyond scope', 'will need to', etc."
                            )
                        } else {
                            String::new()
                        };

                        return Ok(Self::tool_error(format!(
                            "⚠️ VERIFICATION FAILED\n\n\
                            Task {} has a rejected verification with {} issue(s) ({} blocking).\n\n\
                            Summary: {}\n\n\
                            {}{}\n\n\
                            {}",
                            req.id,
                            issue_count,
                            blocking,
                            v.summary,
                            if is_worker_without_subagents {
                                "To fix: Address the issues in this worker.\n\
                                    Then ask supervisor to run verification (task-verifier or direct mcp__cas__verification) and close the task on your behalf."
                                    .to_string()
                            } else {
                                format!(
                                    "To fix: Address the issues and run the {verifier_agent} agent again."
                                )
                            },
                            close_reason_note,
                            if is_worker_without_subagents {
                                format!(
                                    "Suggested message: mcp__cas__coordination action=message target=supervisor message=\"Task {} is ready for re-verification. Please verify (task-verifier or direct mcp__cas__verification) and close if approved.\"",
                                    req.id
                                )
                            } else {
                                format!(
                                    "To verify: Task(subagent_type=\"{}\", prompt=\"Verify task {}\")",
                                    verifier_agent, req.id
                                )
                            }
                        )));
                    }
                    Ok(Some(ref v))
                        if v.status == VerificationStatus::Error
                            && v.summary.starts_with(DISPATCH_SUMMARY_PREFIX)
                            && (chrono::Utc::now() - v.created_at).num_seconds()
                                > VERIFICATION_JAIL_TIMEOUT_SECS =>
                    {
                        // Stale dispatch-request row: the task-verifier subagent was
                        // supposed to write a verdict but never did. This is the
                        // within-task verification deadlock from cas-c29a. Auto-escalate
                        // so the supervisor sees a clean failure instead of an infinite
                        // VERIFICATION REQUIRED loop.
                        let elapsed_mins =
                            (chrono::Utc::now() - v.created_at).num_seconds() / 60;

                        // Clear pending_verification so the jail releases.
                        let mut task_to_update = task.clone();
                        task_to_update.pending_verification = false;
                        task_to_update.updated_at = chrono::Utc::now();
                        if let Err(e) = task_store.update(&task_to_update) {
                            tracing::warn!(task_id = %req.id, error = %e, "failed to clear pending_verification on task");
                        }

                        // Release any lease so the supervisor can reclaim the task.
                        if let Ok(agent_store) = self.open_agent_store() {
                            let _ = agent_store.release_lease_for_task(&req.id);
                        }

                        // Replace the stale dispatch row with a timeout diagnostic so
                        // the audit trail shows escalation instead of a dangling
                        // "Dispatch requested" row.
                        let mut timeout_row = v.clone();
                        timeout_row.summary = format!(
                            "Verification timed out after {elapsed_mins} minutes — \
                             task-verifier subagent never recorded a verdict. \
                             Auto-escalated by cas_task_close: pending_verification cleared, \
                             lease released. Supervisor must re-dispatch verifier or record \
                             verdict manually."
                        );
                        timeout_row.created_at = chrono::Utc::now();
                        if let Err(e) = verification_store.update(&timeout_row) {
                            tracing::warn!(task_id = %req.id, error = %e, "failed to update verification timeout row");
                        }

                        // Surface an activity event so the TUI shows the escalation.
                        if let Ok(agent_id) = self.get_agent_id() {
                            let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                                session_id: agent_id,
                                event_type: "verification_timeout_escalated".to_string(),
                                description: format!(
                                    "Verification timed out ({elapsed_mins}m): {}",
                                    req.id
                                ),
                                entity_id: Some(req.id.clone()),
                            };
                            let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                        }

                        return Ok(Self::tool_error(format!(
                            "⚠️ VERIFICATION TIMED OUT\n\n\
                            Task {} was awaiting verification for {} minutes with no verdict \
                            from the task-verifier subagent. Auto-escalated: verification jail \
                            released, lease freed.\n\n\
                            This usually means the task-verifier subagent crashed, was never \
                            spawned, or failed silently.\n\n\
                            To proceed:\n\
                            1. Re-dispatch verifier: Task(subagent_type=\"task-verifier\", prompt=\"Verify task {}\")\n\
                            2. Or record verdict directly: mcp__cas__verification action=add task_id={} status=approved summary=\"...\"\n\
                            3. Then call cas_task_close again.",
                            req.id, elapsed_mins, req.id, req.id
                        )));
                    }
                    Ok(None) | Ok(Some(_)) => {
                        // No verification or pending/error status
                        // Only auto-claim if the closing agent is the task's assignee.
                        // If a supervisor closes a worker's task, skip the lease to avoid
                        // locking the task to the supervisor.
                        let is_assignee = self
                            .get_agent_id()
                            .ok()
                            .map(|aid| task.assignee.as_deref() == Some(aid.as_str()))
                            .unwrap_or(false);
                        if is_assignee {
                            self.auto_claim_for_verification(&req.id, task_store.as_ref())?;
                        }

                        // Set pending_verification flag to enable verification jail
                        let mut task_to_update = task.clone();
                        task_to_update.pending_verification = true;
                        if task_to_update.assignee.is_none() {
                            if let Ok(agent_id) = self.get_agent_id() {
                                task_to_update.assignee = Some(agent_id);
                            }
                        }
                        // cas-3086: persist the worker's ReviewOutcome envelope on
                        // the task deliverables so a subsequent supervisor close
                        // (once verification approves) can forward the prior review
                        // receipt into the P0 gate instead of re-running the
                        // multi-persona reviewer or requiring `bypass_code_review`.
                        // We persist only non-empty envelopes; validation happens
                        // later in `run_code_review_gate`, which rejects malformed
                        // persisted envelopes so bad input cannot silently bypass
                        // the gate.
                        if let Some(envelope) = req
                            .code_review_findings
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            task_to_update.deliverables.review_envelope =
                                Some(envelope.to_string());
                        }
                        task_to_update.updated_at = chrono::Utc::now();
                        if let Err(e) = task_store.update(&task_to_update) {
                            tracing::warn!(task_id = %req.id, error = %e, "failed to set pending_verification on task");
                        }

                        // Include close reason in the message so verifier can check it
                        let close_reason_section = if let Some(ref reason) = req.reason {
                            format!(
                                "\n\n## Proposed Close Reason\n\
                                ```\n{reason}\n```\n\n\
                                IMPORTANT: The {verifier_agent} MUST validate this close reason.\n\
                                Reject if it admits incomplete work (e.g., 'remaining items', 'beyond scope', 'will need to')."
                            )
                        } else {
                            String::new()
                        };

                        let verification_desc = if is_epic {
                            "Epic verification runs on master to verify the complete merged implementation.\n\
                            The agent will check that all subtask implementations integrate correctly.\n\
                            The verifier MUST record verification_type=epic."
                        } else {
                            "The agent will check for TODO comments, stubs, incomplete implementations,\n\
                            AND validate the close reason doesn't admit incomplete work."
                        };

                        // Send verification blocked activity event (for supervisor visibility)
                        if let Ok(agent_id) = self.get_agent_id() {
                            let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                                session_id: agent_id,
                                event_type: "worker_verification_blocked".to_string(),
                                description: format!("Awaiting verification: {}", req.id),
                                entity_id: Some(req.id.clone()),
                            };
                            let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                        }

                        // Persist a durable dispatch-request row so the close
                        // attempt is observable (in tests, in the UI, and in
                        // audit trails) instead of fire-and-forget text. The
                        // task-verifier subagent will later write its verdict
                        // as a newer row; get_latest_for_task returns the
                        // newest, so behavior on retry is unchanged. Only
                        // create the row on the first attempt — don't
                        // duplicate on repeated close calls.
                        if !had_prior_verification {
                            if let Ok(ver_id) = verification_store.generate_id() {
                                let mut dispatch_row =
                                    Verification::new(ver_id, req.id.clone());
                                dispatch_row.verification_type = verification_type;
                                dispatch_row.status = VerificationStatus::Error;
                                if let Ok(agent_id) = self.get_agent_id() {
                                    dispatch_row.agent_id = Some(agent_id);
                                }
                                dispatch_row.summary = format!(
                                    "Dispatch requested — task-verifier subagent must be spawned via \
                                     Task(subagent_type=\"task-verifier\", prompt=\"Verify task {}\"). \
                                     This row will be superseded by the subagent's verdict.",
                                    req.id
                                );
                                if let Err(e) = verification_store.add(&dispatch_row) {
                                    tracing::warn!(task_id = %req.id, error = %e, "failed to persist verification dispatch row");
                                }
                            }
                        }

                        let verification_gate = if is_factory_worker {
                            format!(
                                "🔒 Factory worker verification gate: this close will only succeed after a task-verifier records a verdict.\n\n\
                                 Spawn the verifier now (other tools remain available while it runs):\n\n\
                                 Task(subagent_type=\"{}\", prompt=\"Verify task {}\")",
                                verifier_agent, req.id
                            )
                        } else if supervisor_is_assignee {
                            format!(
                                "You implemented this task yourself. Spawn a task-verifier to review your work:\n\n\
                                 Task(subagent_type=\"{}\", prompt=\"Verify task {}\")\n\n\
                                 Or record verification directly:\n\
                                 mcp__cas__verification action=add task_id={} status=approved summary=\"Self-verified: <reason>\"",
                                verifier_agent, req.id, req.id
                            )
                        } else {
                            format!(
                                "🔒 VERIFICATION JAIL ACTIVE: You cannot use other tools until you verify this task.\n\n\
                                 Use the Task tool to spawn a task-verifier subagent: \
                                 Task(subagent_type=\"{}\", prompt=\"Verify task {}\")",
                                verifier_agent, req.id
                            )
                        };

                        return Ok(Self::tool_error(format!(
                            "⚠️ VERIFICATION REQUIRED\n\n\
                            Task {} requires verification before closing.\n\n\
                            {}{}\n\n\
                            {}{}\n\n\
                            {}",
                            req.id,
                            verification_gate,
                            verification_desc,
                            close_reason_section.as_str(),
                            if is_worker_without_subagents {
                                format!(
                                    "Ask supervisor to run verification (task-verifier or direct mcp__cas__verification) and close task {} on your behalf.",
                                    req.id
                                )
                            } else {
                                String::new()
                            },
                            if is_worker_without_subagents {
                                format!(
                                    "Suggested message: mcp__cas__coordination action=message target=supervisor message=\"Please verify task {} (task-verifier or direct mcp__cas__verification) and close it if approved.\"",
                                    req.id
                                )
                            } else {
                                "After verification passes, call cas_task_close again.".to_string()
                            }
                        )));
                    }
                    Err(_) => {
                        // Verification store error, proceed anyway
                    }
                }
            }
        }

        // Check for worktree that needs merging (only for epics or tasks with worktrees)
        // This check happens AFTER verification passes
        if let Some(worktree_id) = &task.worktree_id {
            let config = self.load_config();

            // Only trigger jail if worktrees are enabled and require_merge_on_epic_close is true
            let should_check_worktree = config
                .worktrees
                .as_ref()
                .map(|wc| wc.enabled && wc.require_merge_on_epic_close)
                .unwrap_or(false);

            if should_check_worktree {
                if let Ok(wt_store) = self.open_worktree_store() {
                    if let Ok(worktree) = wt_store.get(worktree_id) {
                        // Check if worktree still exists, is active, and hasn't been merged
                        // Skip jail if: removed, merged status, or has merged_at timestamp
                        let needs_merge = worktree.removed_at.is_none()
                            && worktree.status == WorktreeStatus::Active
                            && worktree.merged_at.is_none();

                        if needs_merge {
                            // Set pending_worktree_merge flag to enable worktree jail
                            let mut task_to_update = task.clone();
                            task_to_update.pending_worktree_merge = true;
                            if task_to_update.assignee.is_none() {
                                if let Ok(agent_id) = self.get_agent_id() {
                                    task_to_update.assignee = Some(agent_id);
                                }
                            }
                            task_to_update.updated_at = chrono::Utc::now();
                            if let Err(e) = task_store.update(&task_to_update) {
                                tracing::warn!(task_id = %req.id, error = %e, "failed to set pending_worktree_merge on task");
                            }

                            return Ok(Self::tool_error(format!(
                                "⚠️ WORKTREE MERGE REQUIRED\n\n\
                                Task {} has an associated worktree that needs to be merged before closing.\n\n\
                                📍 Worktree: {}\n\
                                🌿 Branch: {}\n\n\
                                🔒 WORKTREE JAIL ACTIVE: You cannot use other tools until you spawn the 'worktree-merger' agent.\n\n\
                                To merge: Spawn the 'worktree-merger' agent to:\n\
                                1. Check for uncommitted changes and commit them\n\
                                2. Push the branch to remote\n\
                                3. Merge the branch to the parent branch\n\
                                4. Clean up the worktree directory\n\n\
                                After the merge completes, call cas_task_close again.",
                                req.id,
                                worktree.path.display(),
                                worktree.branch
                            )));
                        }
                    }
                }
            }
        }

        // cas-895d + cas-bc1b (follow-up): close-gate checks that inspect
        // the worker's worktree are scoped *only* to tasks with an
        // isolated worker worktree (`task.worktree_id` set).
        //
        // Non-isolated tasks (`isolate=false` in spawn_workers) run
        // directly in the main cas-src worktree, which is routinely
        // dirty during an active session: supervisor edits in flight,
        // shared ops editing shared files, or simply unrelated drift.
        // Running either close gate against the main worktree would
        // reject every close in that mode and reintroduce the exact
        // wrong-worktree-scope bug cas-bc1b was filed to fix.
        //
        // `resolve_worker_worktree_path` returns `None` for non-isolated
        // tasks, and both gates below key off that Option to decide
        // whether to fire at all. For non-isolated tasks the close
        // path relies on cas-code-review (cas-b39f) + verification
        // (task-verifier) as the quality bar — those gates operate on
        // commits / review envelopes, not on working-tree state, so
        // they're safe to run in a shared worktree.
        let bypass_close_gates =
            req.bypass_code_review.unwrap_or(false) && is_supervisor_from_env();
        let worker_worktree_path = self.resolve_worker_worktree_path(&task);

        // cas-895d: uncommitted work gate.
        //
        // The pre-cas-895d close path had no backstop checking that the
        // worker's claimed deliverables were actually committed. A
        // worker could complete a task, run tests, hit `task.close`, pass
        // verification, and successfully close — all while leaving the
        // actual edits **uncommitted** in the working tree. When the
        // worker's isolated worktree was later GC'd, the work was lost.
        //
        // The gate runs `git status --porcelain` scoped to the worker's
        // own worktree. Any non-`??` status line counts as uncommitted
        // tracked work — untracked files (`??`) are ignored because
        // they never belonged to the task in the first place.
        //
        // Scope: tasks with a resolved worker worktree only. Non-
        // isolated tasks skip this gate entirely per the comment above.
        //
        // Supervisors can bypass this gate with `bypass_code_review=true`,
        // matching the same "trust me" pattern used by the cas-b39f
        // code-review gate. Non-supervisors get a hard reject pointing
        // them at the dirty files.
        //
        // Graceful degradation: if the worktree path is not a git repo
        // or git fails, the check silently no-ops. The gate is advisory
        // when git state is unknowable.
        if !bypass_close_gates {
            if let Some(worker_wt) = worker_worktree_path.as_ref() {
                let uncommitted = check_uncommitted_work(worker_wt);
                if !uncommitted.is_empty() {
                    let file_list = uncommitted
                        .iter()
                        .map(|u| format!("  {}  {}", u.status, u.path))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(Self::tool_error(format!(
                        "⚠️ UNCOMMITTED WORK\n\n\
                        task close rejected: the worker's tree has uncommitted tracked \
                        changes. Closing now would lose the work when the worktree is \
                        cleaned up.\n\n\
                        📂 Checked worktree: {}\n\n\
                        Dirty files:\n{file_list}\n\n\
                        To resolve:\n\
                        1. Review the diff: `git status`\n\
                        2. Stage and commit your changes with a meaningful message.\n\
                        3. Re-run `mcp__cas__task action=close id={}`.\n\n\
                        Supervisors may bypass this gate with bypass_code_review=true \
                        (logged as a decision note) when the worker is stuck and the \
                        work on disk is genuinely disposable.",
                        worker_wt.display(),
                        req.id
                    )));
                }
            }
        }

        // cas-e235 + cas-bc1b: additive-only execution_note backstop.
        //
        // If the worker declared `execution_note=additive-only`, reject
        // the close if git sees any modified, deleted, or renamed files
        // in the task's committed history.
        //
        // cas-bc1b: pre-fix, this check ran
        // `git diff --name-status HEAD` inside `self.cas_root.parent()`
        // (the *main* worktree) regardless of whether the task had an
        // attached worker worktree. Two cascading problems:
        //
        // 1. Factory workers commit their work on an isolated branch.
        //    The main worktree's `git status` has **no semantic
        //    relationship** to what the worker did — a stray dirty
        //    `Cargo.lock` in the main repo would fail an
        //    `additive-only` close on a pristine worker branch (the
        //    cas-4333 incident).
        // 2. Workers who do the right thing and commit everything on
        //    their branch produce an empty `git diff HEAD` inside
        //    their own worktree too, because the commits aren't
        //    "uncommitted diff". So the gate wouldn't see violations
        //    even in the correct worktree.
        //
        // Fix (option (a) from the task description): diff the worker
        // branch's committed history against its parent-branch merge
        // base (`git diff <parent>...HEAD` inside the worker's
        // worktree). Commits only — immune to CWD confusion.
        //
        // Non-isolated tasks skip this gate entirely: there's no
        // distinct worker branch to diff against `main`, so the check
        // has nothing to reason about. Earlier iterations fell through
        // to a legacy `git diff HEAD` path on the main worktree — that
        // path has been deleted in this commit because it reintroduced
        // the exact wrong-worktree-scope bug cas-bc1b was filed to fix.
        if task.execution_note.as_deref() == Some("additive-only") {
            if let Some(worker_wt) = worker_worktree_path.as_ref() {
                let parent_branch = task
                    .worktree_id
                    .as_deref()
                    .and_then(|wt_id| {
                        self.open_worktree_store()
                            .ok()
                            .and_then(|store| store.get(wt_id).ok())
                            .map(|wt| wt.parent_branch.clone())
                    })
                    .unwrap_or_else(|| "main".to_string());
                let violations =
                    check_additive_only_branch_violations(worker_wt, &parent_branch);
                if !violations.is_empty() {
                    let file_list = violations
                        .iter()
                        .map(|v| format!("  {} ({})", v.path, v.status))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(Self::tool_error(format!(
                        "⚠️ ADDITIVE-ONLY VIOLATION\n\n\
                        task close rejected: execution_note=additive-only but diff contains \
                        modifications.\n\n\
                        Modified/deleted/renamed files:\n{file_list}\n\n\
                        Use execution_note=null or test-first to modify existing files."
                    )));
                }
            }
        }

        // cas-b39f: cas-code-review P0 close gate (Unit 9).
        //
        // This is the integration point for the multi-persona code review
        // pipeline. The *dispatch* of the review skill itself happens via
        // the worker's harness (the skill must be invoked through the
        // Task tool by an LLM, not from Rust), so the Phase 1 gate works
        // in three cooperating layers:
        //
        //   1. Skip conditions (here) — additive-only tasks, non-code
        //      diffs, and supervisor overrides bypass the gate before
        //      any review is attempted.
        //   2. The pure-Rust decision helper at
        //      `cas_store::code_review::close_gate::evaluate_gate` —
        //      given a residual finding set, returns Allow or
        //      BlockOnP0. Exhaustively unit-tested there.
        //   3. Graceful degradation — if the review pipeline is
        //      unavailable (skill not installed, orchestrator crash,
        //      no findings-cache entry), log a warning and allow the
        //      close. The task description is explicit: code review
        //      must not become a SPOF for closes.
        //
        // Supervisor override flow:
        //   * Caller sets `bypass_code_review=true` on the close
        //     request.
        //   * If `CAS_AGENT_ROLE=supervisor`, the gate is skipped and
        //     a decision note is appended to the task capturing who
        //     overrode and the close reason.
        //   * Any other caller setting the flag gets an explicit
        //     rejection — we do not silently ignore unauthorized
        //     overrides because that would mask a misconfigured
        //     harness.
        let close_project_root = self.cas_root.parent().unwrap_or(&self.cas_root);

        // cas-3086: Epic-close should not re-gate on the union diff
        // when every subtask already carries a valid ReviewOutcome
        // receipt (persisted on deliverables.review_envelope). The
        // subtasks were each individually reviewed before their own
        // close; running the multi-persona reviewer on the unioned
        // diff is redundant cost and wrong-shape signal.
        let epic_subtask_receipts_cover = if task.task_type == TaskType::Epic {
            match task_store.get_subtasks(&req.id) {
                Ok(subtasks) => epic_subtask_receipts_are_clean(&subtasks),
                Err(_) => false,
            }
        } else {
            false
        };

        let gate_outcome = if epic_subtask_receipts_cover {
            CodeReviewGateOutcome::Proceed
        } else {
            run_code_review_gate(&task, &req, close_project_root)
        };
        match gate_outcome {
            CodeReviewGateOutcome::Proceed => {}
            CodeReviewGateOutcome::AppendDecisionNote(note) => {
                let mut t = task.clone();
                if t.notes.is_empty() {
                    t.notes = note;
                } else {
                    t.notes = format!("{}\n\n{}", t.notes, note);
                }
                t.updated_at = chrono::Utc::now();
                if let Err(e) = task_store.update(&t) {
                    tracing::warn!(task_id = %req.id, error = %e, "failed to append code review decision note");
                }
            }
            CodeReviewGateOutcome::Reject(msg) => {
                return Ok(Self::tool_error(msg));
            }
        }

        // Proceed with close
        let mut task = task;
        let now = chrono::Utc::now();
        task.status = TaskStatus::Closed;
        task.closed_at = Some(now);
        task.updated_at = now;

        // Capture deliverables on close
        let mut deliverables = task.deliverables.clone();
        if let Some(worktree_id) = &task.worktree_id {
            if let Ok(wt_store) = self.open_worktree_store() {
                if let Ok(worktree) = wt_store.get(worktree_id) {
                    if let Some(commit) = worktree.merge_commit.clone() {
                        deliverables.merge_commit = Some(commit);
                    }
                }
            }
        }
        task.deliverables = deliverables;

        // When closing via the supervisor bypass (assignee inactive / orphaned /
        // supervisor-forced), we skip the verification gate but MUST still
        // write a durable `Skipped` verification row. Without this row, the
        // MCP jail (`check_pending_verification`) treats the task as
        // unverified and blocks every downstream worker that inherits a
        // BlockedBy on this task. See cas-82d6.
        //
        // cas-3bd4: the Skipped row now records the *actual* skip reason
        // (from `VerificationSkipReason::audit_reason`) instead of the
        // catch-all "assignee inactive or orphaned task" string.
        if skip_verification && verification_enabled {
            if let Ok(verification_store) = self.open_verification_store() {
                let needs_row = verification_store
                    .get_latest_for_task(&req.id)
                    .map(|v| {
                        !matches!(
                            v,
                            Some(ref r) if r.status == VerificationStatus::Approved
                                || r.status == VerificationStatus::Skipped
                        )
                    })
                    .unwrap_or(true);
                if needs_row {
                    if let Ok(ver_id) = verification_store.generate_id() {
                        let mut row = Verification::skipped(
                            ver_id,
                            req.id.clone(),
                            skip_reason.audit_reason(),
                        );
                        row.verification_type = if task.task_type == TaskType::Epic {
                            VerificationType::Epic
                        } else {
                            VerificationType::Task
                        };
                        if let Ok(agent_id) = self.get_agent_id() {
                            row.agent_id = Some(agent_id);
                        }
                        if let Err(e) = verification_store.add(&row) {
                            tracing::warn!(task_id = %req.id, error = %e, "failed to persist verification skip row");
                        }
                    }
                }
            }
        }

        if let Some(reason) = &req.reason {
            task.close_reason = Some(reason.clone());
            let timestamp = now.format("%Y-%m-%d %H:%M");
            let close_note = format!("[{timestamp}] Closed: {reason}");
            if task.notes.is_empty() {
                task.notes = close_note;
            } else {
                task.notes = format!("{}\n\n{}", task.notes, close_note);
            }
        }

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // Auto-unblock tasks that were blocked solely by this task.
        // This keeps dependency state and task status synchronized.
        let mut auto_unblocked_tasks: Vec<String> = Vec::new();
        if let Ok(dependents) = task_store.get_dependents(&req.id) {
            for dep in dependents
                .iter()
                .filter(|dep| dep.dep_type == DependencyType::Blocks)
            {
                if let Ok(mut dependent_task) = task_store.get(&dep.from_id) {
                    if dependent_task.status != TaskStatus::Blocked {
                        continue;
                    }
                    let is_unblocked = task_store
                        .get_blockers(&dependent_task.id)
                        .map(|blockers| blockers.is_empty())
                        .unwrap_or(false);
                    if !is_unblocked {
                        continue;
                    }
                    dependent_task.status = TaskStatus::Open;
                    dependent_task.updated_at = chrono::Utc::now();
                    let timestamp = dependent_task.updated_at.format("%Y-%m-%d %H:%M");
                    let unblock_note = format!(
                        "[{}] Auto-unblocked: all blockers closed (latest: {}).",
                        timestamp, req.id
                    );
                    if dependent_task.notes.is_empty() {
                        dependent_task.notes = unblock_note;
                    } else {
                        dependent_task.notes =
                            format!("{}\n\n{}", dependent_task.notes, unblock_note);
                    }
                    if task_store.update(&dependent_task).is_ok() {
                        auto_unblocked_tasks.push(dependent_task.id.clone());
                    }
                }
            }
        }

        // Track epic completion with subtask count and duration
        if task.task_type == TaskType::Epic {
            let subtasks = task_store.get_subtasks(&req.id).unwrap_or_default();
            let duration_mins = task
                .closed_at
                .zip(Some(task.created_at))
                .map(|(closed, created)| (closed - created).num_minutes().max(0) as u64)
                .unwrap_or(0);
            crate::telemetry::track_epic_completed(subtasks.len(), duration_mins);
        }

        // Release any lease on this task (regardless of who owns it)
        let lease_msg = if let Ok(agent_store) = self.open_agent_store() {
            match agent_store.release_lease_for_task(&req.id) {
                Ok(true) => " (lease released)",
                Ok(false) => "",
                Err(_) => "",
            }
        } else {
            ""
        };

        // cas-3bd4: use the typed skip reason so the audit suffix cites
        // the real reason (e.g. "assignee unknown" for name/id mismatches,
        // "supervisor bypass" for explicit overrides) instead of always
        // saying "assignee inactive".
        let verification_note = skip_reason.response_suffix(verification_enabled);

        // Note about worktree status (merge already handled by worktree-merger agent)
        let worktree_msg = if let Some(worktree_id) = &task.worktree_id {
            if let Ok(wt_store) = self.open_worktree_store() {
                if let Ok(worktree) = wt_store.get(worktree_id) {
                    if worktree.removed_at.is_some() {
                        // Worktree was merged and cleaned up by worktree-merger
                        format!("\n🌳 Worktree merged (branch: {})", worktree.branch)
                    } else {
                        // Worktree still exists - this shouldn't happen if jail worked correctly
                        format!("\n⚠️ Worktree still exists at {}", worktree.path.display())
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Check if this task is a subtask of an epic, and if all siblings are now closed
        let epic_close_msg = {
            // Get dependencies of this task to find its parent
            let deps = task_store.get_dependencies(&req.id).unwrap_or_default();
            let parent_dep = deps
                .iter()
                .find(|d| d.dep_type == cas_types::DependencyType::ParentChild);

            if let Some(dep) = parent_dep {
                // Get the parent task
                if let Ok(parent) = task_store.get(&dep.to_id) {
                    // Check if parent is an Epic
                    if parent.task_type == cas_types::TaskType::Epic
                        && parent.status != TaskStatus::Closed
                    {
                        // Get all subtasks of this epic
                        let subtasks = task_store.get_subtasks(&parent.id).unwrap_or_default();

                        // Check if all subtasks are now closed
                        let all_closed = subtasks.iter().all(|t| t.status == TaskStatus::Closed);

                        if all_closed && !subtasks.is_empty() {
                            // In factory mode, workers shouldn't close epics - supervisor handles that
                            let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
                                .map(|r| r.to_lowercase() == "worker")
                                .unwrap_or(false);

                            if is_factory_worker {
                                // Send real notification to supervisor via daemon event
                                if let Ok(agent_id) = self.get_agent_id() {
                                    let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                                        session_id: agent_id,
                                        event_type: "epic_subtasks_complete".to_string(),
                                        description: format!(
                                            "All subtasks of epic '{}' ({}) are complete — ready to close",
                                            parent.title, parent.id
                                        ),
                                        entity_id: Some(parent.id.clone()),
                                    };
                                    let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                                }

                                format!(
                                    "\n\n🎉 All subtasks of epic '{}' ({}) are now complete!\n\
                                     → The supervisor has been notified to close the epic.",
                                    parent.title, parent.id
                                )
                            } else {
                                format!(
                                    "\n\n🎉 All subtasks of epic '{}' ({}) are now complete!\n\
                                     → Consider closing the epic with: mcp__cas__task action=close id={}",
                                    parent.title, parent.id, parent.id
                                )
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        };

        // Check if commit nudge is enabled
        let commit_nudge = config.tasks().commit_nudge_on_close;
        let commit_nudge_msg =
            if commit_nudge && worktree_msg.is_empty() && epic_close_msg.is_empty() {
                "\n\n💡 Consider committing your changes for this completed task."
            } else {
                ""
            };

        let auto_unblock_msg = if auto_unblocked_tasks.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n🔓 Auto-unblocked task(s): {}",
                auto_unblocked_tasks.join(", ")
            )
        };

        Ok(Self::success(format!(
            "Closed task: {} - {}{}{}{}{}{}{}",
            req.id,
            task.title,
            verification_note,
            lease_msg,
            worktree_msg,
            epic_close_msg,
            commit_nudge_msg,
            auto_unblock_msg
        )))
    }

    /// Resolve the filesystem path for the **worker's isolated
    /// worktree**, if this task has one.
    ///
    /// Returns `Some(worktree_path)` for factory tasks spawned with
    /// `isolate=true` — there's a distinct git worktree per worker,
    /// resolved from `task.worktree_id` → `WorktreeStore`. This is the
    /// only surface where worktree-scoped close gates
    /// (cas-895d uncommitted-work, cas-bc1b additive-only) should fire.
    ///
    /// Returns `None` in three cases, all of which mean "this task does
    /// not have a worker-owned worktree to check":
    ///
    /// 1. `task.worktree_id` is absent entirely — the task ran directly
    ///    in the main cas-src worktree (`isolate=false`), which is
    ///    routinely dirty during an active session. The close gates
    ///    must skip such tasks; checking the main worktree would
    ///    reject every close because of unrelated in-flight work from
    ///    the supervisor or other non-isolated workers. This was the
    ///    pre-cas-895d/cas-bc1b follow-up bug.
    /// 2. The worktree store can't be opened — treat as unknown state
    ///    rather than falling back to the main worktree.
    /// 3. The worktree row exists but has been `removed_at` or its
    ///    on-disk path no longer exists — the worktree was already
    ///    cleaned up, so there's nothing to inspect.
    ///
    /// Callers MUST NOT fall back to `self.cas_root.parent()` on
    /// `None`. The whole point of returning Option is that "no worker
    /// worktree" is different from "the main repo". Any gate that
    /// can't reason without a worker worktree should skip itself.
    pub(crate) fn resolve_worker_worktree_path(
        &self,
        task: &cas_types::Task,
    ) -> Option<std::path::PathBuf> {
        let worktree_id = task.worktree_id.as_deref()?;
        let wt_store = self.open_worktree_store().ok()?;
        let wt = wt_store.get(worktree_id).ok()?;
        if wt.removed_at.is_some() {
            return None;
        }
        if !wt.path.exists() {
            return None;
        }
        Some(wt.path.clone())
    }

    /// Compute why (if at all) the task-verifier step should be skipped
    /// for this close attempt.
    ///
    /// Only invoked after the caller has been identified as a supervisor
    /// and `verification_enabled` is true — the `VerificationSkipReason::None`
    /// cases here represent "supervisor is closing, but the assignee is
    /// still alive and no bypass flag was set, so run the verifier".
    ///
    /// Resolution order:
    ///
    /// 1. No assignee at all → `NoAssignee`.
    /// 2. Consult the task's active lease via `agent_store.get_lease`.
    ///    `TaskLease.agent_id` is the real session-id even when
    ///    `task.assignee` stores a display name, so this is the most
    ///    reliable liveness source. If the lease is valid and the
    ///    referenced agent is alive+fresh → not a skip (unless the
    ///    supervisor passed `bypass_code_review=true`, in which case
    ///    we honor it as `SupervisorBypass`). If the lease is stale or
    ///    the referenced agent is dead → `AssigneeInactive`.
    /// 3. No lease — try a direct `agent_store.get(task.assignee)` for
    ///    legacy tasks whose assignee field may hold an agent_id. Same
    ///    liveness logic as above.
    /// 4. Everything failed → `AssigneeUnknown` (never falsely reported
    ///    as "assignee inactive" — the agent row is simply missing).
    pub(crate) fn compute_verification_skip_reason(
        &self,
        task: &cas_types::Task,
        req: &TaskCloseRequest,
    ) -> VerificationSkipReason {
        let Some(assignee) = task.assignee.as_deref() else {
            return VerificationSkipReason::NoAssignee;
        };

        let Ok(agent_store) = self.open_agent_store() else {
            // Can't reach the agent store at all — be conservative and
            // let verification run (None is the safe default).
            return VerificationSkipReason::None;
        };

        let bypass_requested = req.bypass_code_review.unwrap_or(false);
        let alive_result = |agent: &cas_types::Agent| {
            agent.is_alive() && !agent.is_heartbeat_expired(ASSIGNEE_STALE_SECS)
        };
        let stale_minutes = |agent: &cas_types::Agent| {
            chrono::Utc::now()
                .signed_duration_since(agent.last_heartbeat)
                .num_minutes()
        };

        // 1) Lease-based path. TaskLease.agent_id always holds the real
        //    session id, so this survives the name-vs-id mismatch that
        //    broke the pre-cas-3bd4 path.
        if let Ok(Some(lease)) = agent_store.get_lease(&task.id) {
            if lease.is_valid() {
                if let Ok(agent) = agent_store.get(&lease.agent_id) {
                    return if alive_result(&agent) {
                        if bypass_requested {
                            VerificationSkipReason::SupervisorBypass
                        } else {
                            VerificationSkipReason::None
                        }
                    } else {
                        VerificationSkipReason::AssigneeInactive {
                            minutes_stale: Some(stale_minutes(&agent)),
                        }
                    };
                }
                // Lease is valid but the referenced agent row is gone —
                // agent was unregistered but the lease wasn't cleaned up.
                return VerificationSkipReason::AssigneeUnknown;
            }
            // Lease exists but expired.
            return VerificationSkipReason::AssigneeInactive {
                minutes_stale: None,
            };
        }

        // 2) No lease — try the legacy direct-id lookup. Works only when
        //    task.assignee holds an agent_id, not a display name.
        if let Ok(agent) = agent_store.get(assignee) {
            return if alive_result(&agent) {
                if bypass_requested {
                    VerificationSkipReason::SupervisorBypass
                } else {
                    VerificationSkipReason::None
                }
            } else {
                VerificationSkipReason::AssigneeInactive {
                    minutes_stale: Some(stale_minutes(&agent)),
                }
            };
        }

        // 3) No lease, no matching agent row. The assignee is unknown
        //    to the store — do not falsely report "inactive".
        VerificationSkipReason::AssigneeUnknown
    }

    /// Reopen a closed task
    pub async fn cas_task_reopen(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if task.status != TaskStatus::Closed {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "Task is already {} (only closed tasks can be reopened)",
                    task.status
                ),
            ));
        }

        task.status = TaskStatus::Open;
        task.closed_at = None;
        task.updated_at = chrono::Utc::now();

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!(
            "Reopened task: {} - {}",
            req.id, task.title
        )))
    }
}

/// A single additive-only violation: a file whose git status indicates it
/// was modified, deleted, or renamed relative to HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdditiveOnlyViolation {
    pub status: String,
    pub path: String,
}

/// A single uncommitted-work entry: a tracked file that `git status` reports
/// as modified, deleted, added-but-not-committed, renamed, or copied.
///
/// `status` is the raw two-char porcelain field (e.g. ` M`, `M `, `A `,
/// `D `, `R `) and `path` is the workspace-relative path git reported.
/// Untracked (`??`) entries are excluded by [`check_uncommitted_work`];
/// they never belonged to the task in the first place so they cannot
/// represent lost work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncommittedEntry {
    pub status: String,
    pub path: String,
}

/// Return tracked files that are modified, staged, or otherwise in a
/// non-committed state relative to HEAD in the git repo at
/// `project_root`. Returns an empty vec for non-git directories or if
/// the `git` subprocess fails — the gate is advisory and must not
/// block closes it cannot reason about.
///
/// The check is deliberately scoped to **tracked** files. Untracked
/// files (`??`) are allowed through because:
///   * They're safe to delete if the task is disposable.
///   * They're often scratch output (`*.log`, `target/`) that the
///     worker had no intention of committing.
///   * If the worker *did* intend to commit them, they would have run
///     `git add` first, which promotes them to the `A ` status and the
///     gate catches them.
pub(crate) fn check_uncommitted_work(project_root: &std::path::Path) -> Vec<UncommittedEntry> {
    use std::process::Command;

    let Ok(output) = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_root)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // Porcelain format: "XY path" where XY is a 2-char status.
        // Short lines, empty lines → skip.
        if line.len() < 4 {
            continue;
        }
        let (status, rest) = line.split_at(2);
        // Skip untracked entries (`??`). They're additive by nature
        // and never represent a lost commit.
        if status == "??" {
            continue;
        }
        // Rename format: "R  old -> new". Record the new path.
        let path = if let Some((_, new)) = rest.trim_start().split_once(" -> ") {
            new.to_string()
        } else {
            rest.trim_start().to_string()
        };
        entries.push(UncommittedEntry {
            status: status.to_string(),
            path,
        });
    }
    entries
}

/// cas-bc1b: check additive-only violations by comparing the worker
/// branch's committed history against its parent branch. This is the
/// path used for factory worker tasks — it inspects only what the
/// worker committed on their isolated branch, immune to the
/// main-worktree dirty-state confusion that tripped cas-4333.
///
/// Runs `git diff --name-status <merge-base>..HEAD` inside
/// `worker_worktree_path` and filters to M/D/R statuses via
/// [`parse_name_status`]. Untracked files don't exist in committed
/// history, so `??` handling isn't needed here.
///
/// Graceful degradation: if the worktree isn't a git repo, git can't
/// find `parent_branch`, or the merge-base computation fails, returns
/// an empty vec. The gate is advisory when git state is unknowable.
pub(crate) fn check_additive_only_branch_violations(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
) -> Vec<AdditiveOnlyViolation> {
    use std::process::Command;

    // Resolve the merge base first. Using `git merge-base` explicitly
    // (rather than the `a..b` revspec shorthand) means we get a clear
    // failure signal if the parent branch ref can't be resolved — we
    // don't silently compare against the wrong thing.
    let merge_base_out = Command::new("git")
        .args(["merge-base", "HEAD", parent_branch])
        .current_dir(worker_worktree_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return Vec::new(),
    };
    if merge_base.is_empty() {
        return Vec::new();
    }

    let diff_out = Command::new("git")
        .args(["diff", "--name-status", &format!("{merge_base}..HEAD")])
        .current_dir(worker_worktree_path)
        .output();
    match diff_out {
        Ok(o) if o.status.success() => {
            parse_name_status(&String::from_utf8_lossy(&o.stdout))
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// cas-95ce: per-task close-time merge-state guard
// ---------------------------------------------------------------------------

/// Outcome of the cas-95ce factory-branch merge-state close gate.
///
/// Mirrors [`CodeReviewGateOutcome`] in shape so the call site is a
/// uniform pattern-match. The gate exposes only `Proceed` / `Reject`
/// because, unlike the cas-code-review gate, this one has no
/// supervisor override path — bypass cannot skip a data-state guard.
#[derive(Debug)]
pub(crate) enum MergeStateGateOutcome {
    /// Close may proceed — factory branch is merged into the parent
    /// epic branch (or the guard is structurally skipped: epic-type
    /// task, no assignee, branch missing locally, or git history
    /// unknowable).
    Proceed,
    /// Close must be rejected with this user-facing error message.
    Reject(String),
}

/// Per-task close-time guard: reject `task.close` when the worker's
/// `factory/<assignee>` branch carries commits not present on the
/// parent epic branch.
///
/// Runs BEFORE [`run_code_review_gate`]; consequently
/// `bypass_code_review=true` cannot skip it. This is a data-state
/// guard, not a review gate — a reviewed-but-unmerged branch is
/// still stranded work, and the entire point of the cas-95ce/cas-754b
/// scoping decision is that there is no escape hatch (an escape
/// hatch would let the same workaround pattern persist that motivated
/// the EPIC).
///
/// Skipped (Proceed) when:
/// - `task.task_type == Epic` — epic close is already covered by
///   [`check_unmerged_epic_branches`] at the epic-id branch namespace.
/// - `task.assignee.is_none()` — orphaned task; nothing to check.
/// - `factory/<assignee>` does not exist locally and merge-base
///   computation fails — graceful pass. We do not false-reject when
///   the worktree predates the convention or the branch was already
///   pruned post-merge.
///
/// Rejects (Reject) when the factory branch has > 0 commits not on
/// `parent_branch`. The error message includes the stranded count,
/// the factory branch name, the parent branch name, and explicit
/// remediation steps.
///
/// `_req` is intentionally unused — the bypass flag does not affect
/// this guard. It is carried through so the call signature mirrors
/// [`run_code_review_gate`] and the structural placement (this gate
/// sits upstream of any bypass evaluation) is self-documenting.
pub(crate) fn run_factory_branch_merge_gate(
    task: &Task,
    _req: &TaskCloseRequest,
    parent_branch: &str,
    repo_path: &std::path::Path,
) -> MergeStateGateOutcome {
    if task.task_type == TaskType::Epic {
        return MergeStateGateOutcome::Proceed;
    }
    let Some(assignee) = task.assignee.as_deref() else {
        return MergeStateGateOutcome::Proceed;
    };
    let factory_branch = format!("factory/{assignee}");
    let stranded =
        count_unmerged_factory_commits(repo_path, &factory_branch, parent_branch);
    if stranded == 0 {
        return MergeStateGateOutcome::Proceed;
    }
    MergeStateGateOutcome::Reject(format!(
        "⚠️ MERGE REQUIRED\n\n\
         task close rejected: {factory_branch} has {stranded} commit(s) not on \
         {parent_branch}.\n\n\
         Push the branch and merge a PR before closing. This guard cannot be \
         bypassed (use of bypass_code_review=true does not skip merge-state \
         checks — it is a data-state guard, not a review gate).\n\n\
         Remediation:\n\
         1. Push {factory_branch} to its remote\n\
         2. Open a PR targeting {parent_branch}\n\
         3. Merge the PR (or `git fetch --prune` if it was already merged \
         and your local ref is stale)\n\
         4. Retry mcp__cas__task action=close",
    ))
}

/// Count commits reachable from `factory_branch` but not from
/// `parent_branch`, within the git repository rooted at `repo_path`.
///
/// Returns 0 (treated as "merged" by [`run_factory_branch_merge_gate`])
/// when:
/// - The factory branch ref does not resolve locally (worker may
///   have pushed-and-pruned, or never pushed in this checkout).
/// - The merge-base between the two refs cannot be computed.
/// - `git rev-list --count` fails or returns an unparseable value.
///
/// Mirrors the shell-out style of
/// [`check_additive_only_branch_violations`] — no external git crate.
pub(crate) fn count_unmerged_factory_commits(
    repo_path: &std::path::Path,
    factory_branch: &str,
    parent_branch: &str,
) -> u32 {
    use std::process::Command;

    // Resolve merge-base explicitly so we get a clean failure signal
    // when either ref can't be resolved (vs. silently comparing
    // against the wrong base via the `a..b` revspec).
    let merge_base_out = Command::new("git")
        .args(["merge-base", parent_branch, factory_branch])
        .current_dir(repo_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => return 0,
    };
    if merge_base.is_empty() {
        return 0;
    }

    let count_out = Command::new("git")
        .args([
            "rev-list",
            "--count",
            &format!("{merge_base}..{factory_branch}"),
        ])
        .current_dir(repo_path)
        .output();
    match count_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u32>()
            // Saturate on overflow so an implausibly large count
            // still maps to "stranded" (Reject), not 0 (Proceed).
            // Unreachable in practice but the semantically correct
            // direction for an unparseable count.
            .unwrap_or(u32::MAX),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// cas-8f8f: epic-close per-child merge-state gate + diagnostic
// ---------------------------------------------------------------------------

/// One row in the epic_status diagnostic / epic-close gate report.
///
/// Captures everything the supervisor needs to see at a glance: which
/// child task this is, who owns it, whether their factory branch has
/// stranded commits relative to the parent epic, and (for unmerged
/// rows) when that branch was last touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EpicChildBranchStatus {
    pub task_id: String,
    pub task_status: TaskStatus,
    pub assignee: Option<String>,
    /// `factory/<assignee>` derived at scan time. `None` when the
    /// child has no assignee (the gate / report skip these — there
    /// is no factory branch convention to check).
    pub factory_branch: Option<String>,
    pub unmerged_count: u32,
    /// Unix epoch seconds of the most recent commit on
    /// `factory_branch`. `None` when the branch ref doesn't resolve
    /// or `git log` fails.
    pub last_commit_unix: Option<i64>,
}

/// Walk an epic's children, derive `factory/<assignee>` for each
/// child with an assignee, and report per-child unmerged-commit
/// counts vs. `parent_branch`.
///
/// Used by both:
/// - `factory_epic_status` (read-only diagnostic — renders all rows)
/// - `run_epic_close_merge_gate` (close gate — filters to rows with
///   `unmerged_count > 0`)
///
/// Children without an assignee are still represented in the output
/// so the report is complete; the gate filters them out by checking
/// `factory_branch.is_some() && unmerged_count > 0`.
pub(crate) fn collect_epic_branch_statuses(
    subtasks: &[Task],
    parent_branch: &str,
    repo_path: &std::path::Path,
) -> Vec<EpicChildBranchStatus> {
    subtasks
        .iter()
        .map(|t| {
            let factory_branch = t.assignee.as_ref().map(|a| format!("factory/{a}"));
            let (unmerged_count, last_commit_unix) = match factory_branch.as_deref() {
                Some(branch) => (
                    count_unmerged_factory_commits(repo_path, branch, parent_branch),
                    last_commit_unix(repo_path, branch),
                ),
                None => (0, None),
            };
            EpicChildBranchStatus {
                task_id: t.id.clone(),
                task_status: t.status,
                assignee: t.assignee.clone(),
                factory_branch,
                unmerged_count,
                last_commit_unix,
            }
        })
        .collect()
}

/// Render the per-child branch statuses as a Markdown report for the
/// supervisor-facing `factory_epic_status` action. Stable shape; the
/// snapshot test in `epic_status_gate_tests` pins the exact layout.
pub(crate) fn render_epic_status_report(
    epic_id: &str,
    parent_branch: &str,
    statuses: &[EpicChildBranchStatus],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Epic {epic_id} — factory branch status\n\
         Parent branch: {parent_branch}\n\n",
    ));
    if statuses.is_empty() {
        out.push_str("(no child tasks)\n");
        return out;
    }
    out.push_str("| Task | Status | Assignee | Factory branch | Unmerged | Last commit |\n");
    out.push_str("|------|--------|----------|----------------|----------|-------------|\n");
    for s in statuses {
        // Use Display (snake_case: in_progress, closed) rather than
        // Debug (PascalCase: InProgress, Closed) so the supervisor-
        // facing column matches the rest of the CLI's status rendering
        // (e.g., `task list`). Round-1 cas-code-review fix.
        let status_str = s.task_status.to_string();
        let assignee = s.assignee.as_deref().unwrap_or("—");
        let branch = s.factory_branch.as_deref().unwrap_or("—");
        let unmerged = if s.factory_branch.is_some() {
            s.unmerged_count.to_string()
        } else {
            "—".to_string()
        };
        let last_commit = match s.last_commit_unix {
            Some(ts) => format_unix_timestamp(ts),
            None => "—".to_string(),
        };
        out.push_str(&format!(
            "| {task} | {status} | {assignee} | {branch} | {unmerged} | {last} |\n",
            task = s.task_id,
            status = status_str,
            assignee = assignee,
            branch = branch,
            unmerged = unmerged,
            last = last_commit,
        ));
    }
    let stranded = statuses.iter().filter(|s| s.unmerged_count > 0).count();
    if stranded > 0 {
        out.push_str(&format!(
            "\n⚠️  {stranded} child task(s) carry stranded factory commits. \
             Epic close will be hard-blocked until they are merged.\n",
        ));
    } else {
        out.push_str(
            "\n✓ All child factory branches are merged into the parent epic branch.\n",
        );
    }
    out
}

/// Format a Unix epoch second as ISO-8601 UTC. Pure helper used only
/// by [`render_epic_status_report`] in this module; tests in the
/// same file can call private helpers directly. (Round-1 cas-code-review
/// fix: was previously `pub(crate)` for no good reason.)
fn format_unix_timestamp(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("ts={ts}"))
}

/// Last-commit timestamp on `branch` (Unix epoch seconds), or `None`
/// when the branch ref doesn't resolve or `git log` fails. Mirrors
/// the shell-out style of [`count_unmerged_factory_commits`].
pub(crate) fn last_commit_unix(
    repo_path: &std::path::Path,
    branch: &str,
) -> Option<i64> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["log", "-1", "--format=%ct", branch])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse::<i64>().ok()
}

/// Outcome of the cas-8f8f epic-close per-child merge-state gate.
///
/// Symmetric to [`MergeStateGateOutcome`] but at the epic scope.
/// Bypass-immune by construction (no bypass parameter on the gate).
#[derive(Debug)]
pub(crate) enum EpicCloseGateOutcome {
    Proceed,
    Reject(String),
}

/// Per-epic close-time guard: reject Epic-task close when ANY child
/// task carries unmerged commits on its `factory/<assignee>` branch.
///
/// Runs IN ADDITION to (and AFTER) the existing
/// [`check_unmerged_epic_branches`] which only validates the epic's
/// own branch namespace. cas-8f8f extends the principle from "epic
/// branch" to "every child task's factory branch".
///
/// Bypass-immune for the same reasons as cas-95ce
/// [`run_factory_branch_merge_gate`]: this is a data-state guard,
/// not a review gate. `_req` is intentionally unused.
pub(crate) fn run_epic_close_merge_gate(
    task: &Task,
    _req: &TaskCloseRequest,
    parent_branch: &str,
    repo_path: &std::path::Path,
    subtasks: &[Task],
) -> EpicCloseGateOutcome {
    if task.task_type != TaskType::Epic {
        return EpicCloseGateOutcome::Proceed;
    }
    let statuses = collect_epic_branch_statuses(subtasks, parent_branch, repo_path);
    let stranded: Vec<&EpicChildBranchStatus> = statuses
        .iter()
        .filter(|s| s.factory_branch.is_some() && s.unmerged_count > 0)
        .collect();
    if stranded.is_empty() {
        return EpicCloseGateOutcome::Proceed;
    }
    let mut detail = String::new();
    for s in &stranded {
        // Round-1 cas-code-review autofix: use idiomatic `writeln!`
        // rather than the explicit `Write::write_fmt(format_args!(...))`
        // desugaring. `writeln!` returns Result; the surrounding
        // String backing cannot fail, so the discard is intentional.
        use std::fmt::Write as _;
        let _ = writeln!(
            detail,
            "  - {task} ({branch}): {n} commit(s) not on {parent}",
            task = s.task_id,
            branch = s.factory_branch.as_deref().unwrap_or("—"),
            n = s.unmerged_count,
            parent = parent_branch,
        );
    }
    EpicCloseGateOutcome::Reject(format!(
        "⚠️ MERGE REQUIRED\n\n\
         Epic {epic_id} cannot close — {n} child task(s) have stranded factory \
         branches:\n{detail}\n\
         Each child's factory branch must be merged into {parent} before the \
         epic can close. This guard cannot be bypassed (use of \
         bypass_code_review=true does not skip merge-state checks — it is a \
         data-state guard, not a review gate).\n\n\
         Diagnostic: run `mcp__cas__coordination action=epic_status id={epic_id}` \
         for a per-child report.",
        epic_id = task.id,
        n = stranded.len(),
        detail = detail,
        parent = parent_branch,
    ))
}

// ---------------------------------------------------------------------------
// cas-3086 + cas-fef4: epic subtask receipts bypass helper
// ---------------------------------------------------------------------------

/// Decide whether an epic's subtasks collectively carry clean review
/// receipts that justify skipping the multi-persona close gate on the
/// union diff.
///
/// Returns `true` iff every subtask:
///   * has a non-empty `deliverables.review_envelope`,
///   * whose JSON deserializes into a [`cas_types::ReviewOutcome`],
///   * passes `ReviewOutcome::validate()`,
///   * has **no PR-introduced P0** in `residual` (cas-3086 defense-in-depth), and
///   * has **no P0 reclassified into `pre_existing`** (cas-fef4 forgery defense).
///
/// Returns `false` when the subtask list is empty — there is nothing to
/// "cover" the union diff, so fall through to the normal gate.
///
/// ## Why both residual- and pre_existing-P0 disqualify the bypass
///
/// The bypass treats "every subtask has a clean receipt" as a proof
/// stand-in that the union diff was already reviewed piece-by-piece. A
/// worker supplying an envelope of shape `{ residual: [], pre_existing:
/// [<real_p0>] }` would satisfy the old `evaluate_gate(residual) ==
/// Allow` check but smuggle a real P0 past the epic-close gate — the
/// `pre_existing` channel was designed to classify *findings that
/// predate the change*, not as a free downgrade slot for workers to
/// drop P0s into. Per cas-fef4, we tighten the clean-receipt semantics
/// to reject any receipt where a P0 appears anywhere — residual OR
/// pre_existing. Legitimate pre-existing P0s on a change's diff are
/// extraordinarily rare; if one genuinely appears post-hoc, re-running
/// the gate is cheap insurance compared with a silent bypass.
///
/// ## Staleness note
///
/// This helper still treats the persisted envelopes structurally — it
/// cannot detect whether the epic branch has commits *not* covered by
/// any subtask's reviewed diff (supervisor fixups, merge-resolution
/// commits). That is tracked separately (cas-cc1d staleness follow-up)
/// and needs a diff-SHA anchor in the envelope schema to close cleanly.
pub(crate) fn epic_subtask_receipts_are_clean(subtasks: &[Task]) -> bool {
    use cas_store::code_review::close_gate::{GateDecision, evaluate_gate};
    use cas_types::FindingSeverity;

    if subtasks.is_empty() {
        return false;
    }

    subtasks.iter().all(|t| {
        t.deliverables
            .review_envelope
            .as_deref()
            .and_then(|e| serde_json::from_str::<cas_types::ReviewOutcome>(e).ok())
            .filter(|o| o.validate().is_ok())
            .map(|o| {
                // cas-3086: no PR-introduced P0 in residual.
                let residual_clean =
                    matches!(evaluate_gate(&o.residual), GateDecision::Allow);
                // cas-fef4: no P0 smuggled through pre_existing.
                let pre_existing_clean = o
                    .pre_existing
                    .iter()
                    .all(|f| f.severity != FindingSeverity::P0);
                residual_clean && pre_existing_clean
            })
            .unwrap_or(false)
    })
}

// ---------------------------------------------------------------------------
// cas-b39f (Unit 9): cas-code-review P0 close gate
// ---------------------------------------------------------------------------

/// Outcome of the cas-code-review close gate, as seen by `cas_task_close`.
///
/// This enum is deliberately tiny: the hard work (P0 residual evaluation)
/// lives in `cas_store::code_review::close_gate::evaluate_gate`, and the
/// soft conditions (supervisor override, additive-only skip, non-code
/// diff, graceful degradation) are resolved by [`run_code_review_gate`]
/// below. The call site in `cas_task_close` just pattern-matches on the
/// three outcomes.
#[derive(Debug)]
pub(crate) enum CodeReviewGateOutcome {
    /// Close may proceed. No note to write, no error to return.
    Proceed,
    /// Close may proceed, but the caller should append this decision
    /// note to the task before the main close transaction. Used for
    /// the supervisor override path so the audit trail captures who
    /// downgraded a P0 block and why.
    AppendDecisionNote(String),
    /// Close must be rejected with this user-facing error message.
    /// Used for (a) P0 residual blocks, and (b) unauthorized override
    /// attempts.
    Reject(String),
}

/// Decide whether the cas-code-review P0 close gate fires for this
/// close request.
///
/// Per brainstorm Outstanding Question #1 option (a): the worker runs
/// the cas-code-review skill *before* calling `task.close` and passes
/// the structured findings envelope in via
/// [`TaskCloseRequest::code_review_findings`]. This Rust helper only
/// enforces the gate on what the worker sends — it does not (and
/// cannot) invoke the skill itself.
///
/// Contract:
///
/// - `execution_note == "additive-only"` → [`Proceed`]. Pure-addition
///   closes are new-files-only by definition and already covered by
///   the cas-e235 gate above.
/// - `bypass_code_review == Some(true)` and caller is a supervisor →
///   [`AppendDecisionNote`] with the override reason. Gate skipped.
/// - `bypass_code_review == Some(true)` and caller is **not** a
///   supervisor → [`Reject`] with an unauthorized-override message.
///   Silently ignoring the flag would mask a misconfigured harness.
/// - `has_reviewable_changes(project_root) == false` → [`Proceed`].
///   Pure docs-only diffs (`*.md` / `docs/**`) and pure test-only
///   diffs do not require a code review pass.
/// - `code_review_findings == None` at this point → [`Reject`] with
///   `CODE_REVIEW_REQUIRED`, pointing the worker at the skill.
/// - `code_review_findings == Some(envelope)` that fails
///   [`ReviewOutcome::validate`] → [`Reject`] as a malformed envelope.
/// - Otherwise → defer to
///   [`cas_store::code_review::close_gate::evaluate_gate`]. Any
///   non-pre-existing P0 in `residual` → [`Reject`] with a formatted
///   block message; else [`Proceed`].
pub(crate) fn run_code_review_gate(
    task: &Task,
    req: &TaskCloseRequest,
    project_root: &std::path::Path,
) -> CodeReviewGateOutcome {
    // Skip 1: additive-only tasks bypass the gate entirely.
    if task.execution_note.as_deref() == Some("additive-only") {
        return CodeReviewGateOutcome::Proceed;
    }

    // Skip 2: supervisor override.
    if req.bypass_code_review.unwrap_or(false) {
        if is_supervisor_from_env() {
            let reason = req
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("(no reason provided)");
            let note = format!(
                "[{}] DECISION: cas-code-review P0 gate overridden by supervisor. \
                 Reason: {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                reason
            );
            return CodeReviewGateOutcome::AppendDecisionNote(note);
        } else {
            return CodeReviewGateOutcome::Reject(
                "⚠️ UNAUTHORIZED OVERRIDE\n\n\
                 task close rejected: bypass_code_review=true is only honored \
                 when the caller runs as a supervisor (CAS_AGENT_ROLE=supervisor). \
                 Non-supervisor callers must either fix the P0 findings and retry \
                 close, or ask a supervisor to issue the override."
                    .to_string(),
            );
        }
    }

    // Skip 3: docs-only / test-only / empty diffs. The gate is not a
    // SPOF for changes it cannot meaningfully review.
    if !has_reviewable_changes(project_root) {
        return CodeReviewGateOutcome::Proceed;
    }

    // From here on, we require a findings envelope. The request's
    // `code_review_findings` always wins; if it is absent or empty we
    // fall back to any envelope persisted on the task deliverables
    // from a prior jailed close (cas-3086). The persisted fallback is
    // *not* a merge — an explicit request envelope wholly replaces
    // what the gate sees.
    let persisted_envelope = task
        .deliverables
        .review_envelope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let envelope_json = match req.code_review_findings.as_deref() {
        Some(s) if !s.trim().is_empty() => s,
        _ => match persisted_envelope {
            Some(s) => s,
            None => {
                return CodeReviewGateOutcome::Reject(
                    "⚠️ CODE_REVIEW_REQUIRED\n\n\
                     task close rejected: this task has reviewable code changes \
                     and no code_review_findings envelope was provided.\n\n\
                     To resolve:\n\
                     1. Invoke the cas-code-review skill via the Skill or Task \
                        tool with mode=autofix and the current diff.\n\
                     2. Collect the returned ReviewOutcome envelope (residual, \
                        pre_existing, mode).\n\
                     3. Re-call task.close with the envelope JSON-stringified \
                        in code_review_findings.\n\n\
                     Supervisors may bypass this gate with \
                     bypass_code_review=true (logged as a decision note)."
                        .to_string(),
                );
            }
        },
    };

    let envelope: cas_types::ReviewOutcome = match serde_json::from_str(envelope_json) {
        Ok(e) => e,
        Err(e) => {
            return CodeReviewGateOutcome::Reject(format!(
                "⚠️ MALFORMED REVIEW ENVELOPE\n\n\
                 task close rejected: code_review_findings failed to parse \
                 as ReviewOutcome JSON: {e}\n\n\
                 Expected shape: {{residual: Finding[], pre_existing: Finding[], mode: string}}."
            ));
        }
    };

    if let Err(e) = envelope.validate() {
        return CodeReviewGateOutcome::Reject(format!(
            "⚠️ MALFORMED REVIEW ENVELOPE\n\n\
             task close rejected: code_review_findings failed validation: {e}\n\n\
             The worker-side cas-code-review skill returned a structurally \
             invalid envelope. Re-run the review and retry close."
        ));
    }

    use cas_store::code_review::close_gate::{GateDecision, evaluate_gate, format_block_message};
    match evaluate_gate(&envelope.residual) {
        GateDecision::Allow => CodeReviewGateOutcome::Proceed,
        GateDecision::BlockOnP0(blocking) => {
            CodeReviewGateOutcome::Reject(format_block_message(&task.id, &blocking))
        }
    }
}

/// Return `true` if `project_root` has any staged, unstaged, or
/// committed-since-base changes in files that are worth asking the
/// multi-persona reviewer about. Returns `false` for docs-only
/// (`*.md`, anything under `docs/`) and test-only diffs, and for
/// non-git directories where we cannot reason about the diff.
///
/// The classification is deliberately *loose*: when we cannot tell
/// whether a change is reviewable, we assume it is, and the worker
/// runs the review. False positives waste latency; false negatives
/// silently skip the gate.
pub(crate) fn has_reviewable_changes(project_root: &std::path::Path) -> bool {
    use std::process::Command;

    // Collect changed paths from both the index/working-tree diff and
    // the HEAD diff. Union handles in-flight edits on top of the
    // already-committed task work.
    let mut changed: Vec<String> = Vec::new();

    for args in [
        &["diff", "--name-only", "HEAD"][..],
        &["diff", "--name-only", "--cached"][..],
    ] {
        if let Ok(output) = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
        {
            if !output.status.success() {
                // Not a git repo, or HEAD doesn't exist — we cannot
                // reason about the diff, so the gate should not block.
                // Per the "not a SPOF" rule, treat as no-reviewable.
                return false;
            }
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    changed.push(trimmed.to_string());
                }
            }
        } else {
            return false;
        }
    }

    changed.sort();
    changed.dedup();

    changed.iter().any(|path| is_reviewable_path(path))
}

/// Classify a single path as "worth running the multi-persona
/// reviewer on". Docs (`*.md`, anything under `docs/`) and tests
/// (anything under `tests/`, `test/`, or a file ending in
/// `_test.rs` / `.test.ts`) are excluded.
pub(crate) fn is_reviewable_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();

    // Docs
    if lower.ends_with(".md") {
        return false;
    }
    if lower.starts_with("docs/") || lower.contains("/docs/") {
        return false;
    }

    // Tests
    if lower.starts_with("tests/") || lower.contains("/tests/") {
        return false;
    }
    if lower.starts_with("test/") || lower.contains("/test/") {
        return false;
    }
    if lower.ends_with("_test.rs")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.go")
    {
        return false;
    }

    true
}

/// Parse the output of `git diff --name-status` into violations. Only rows
/// whose status starts with M, D, or R are returned. A, C, T, U, and ?? are
/// considered additive or uninteresting.
fn parse_name_status(output: &str) -> Vec<AdditiveOnlyViolation> {
    let mut violations = Vec::new();
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // Format: "<STATUS>\t<PATH>" or for renames "R100\t<OLD>\t<NEW>"
        let mut parts = line.splitn(3, '\t');
        let Some(status) = parts.next() else {
            continue;
        };
        let Some(first_path) = parts.next() else {
            continue;
        };
        let second_path = parts.next();
        let first_char = status.chars().next().unwrap_or(' ');
        match first_char {
            'M' | 'D' => violations.push(AdditiveOnlyViolation {
                status: status.to_string(),
                path: first_path.to_string(),
            }),
            'R' => {
                let path = second_path.unwrap_or(first_path).to_string();
                violations.push(AdditiveOnlyViolation {
                    status: status.to_string(),
                    path,
                });
            }
            _ => {}
        }
    }
    violations
}

#[cfg(test)]
mod additive_only_tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("existing.txt"), "original\n").unwrap();
        git(p, &["add", "existing.txt"]);
        git(p, &["commit", "-q", "-m", "initial"]);
        dir
    }

    // Legacy `check_additive_only_violations` unit tests (non_git_dir,
    // clean_repo, new_file, modified_file, deleted_file, renamed_file)
    // were removed alongside the function itself. The `branch_check_*`
    // tests below cover the replacement path — see cas-bc1b follow-up.

    #[test]
    fn parse_name_status_mixed() {
        let out = "A\tadded.txt\nM\tmodified.txt\nD\tdeleted.txt\nR100\told.txt\tnew.txt\n";
        let v = parse_name_status(out);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].path, "modified.txt");
        assert_eq!(v[1].path, "deleted.txt");
        assert_eq!(v[2].path, "new.txt");
        assert!(v[2].status.starts_with('R'));
    }

    // --- cas-895d: check_uncommitted_work ---------------------------------

    #[test]
    fn uncommitted_non_git_dir_is_empty() {
        let dir = tempdir().unwrap();
        assert!(check_uncommitted_work(dir.path()).is_empty());
    }

    #[test]
    fn uncommitted_clean_repo_is_empty() {
        let dir = init_repo();
        assert!(check_uncommitted_work(dir.path()).is_empty());
    }

    #[test]
    fn uncommitted_untracked_file_is_ignored() {
        let dir = init_repo();
        std::fs::write(dir.path().join("scratch.log"), "noise\n").unwrap();
        let v = check_uncommitted_work(dir.path());
        assert!(
            v.is_empty(),
            "untracked files must not count as lost work, got: {v:?}"
        );
    }

    #[test]
    fn uncommitted_staged_new_file_is_caught() {
        // cas-895d core scenario: the worker wrote a new file and staged
        // it, but never committed. This is EXACTLY the cas-953d miss —
        // the work exists on disk but would be GC'd with the worktree.
        let dir = init_repo();
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1, "staged-but-uncommitted must block: {v:?}");
        assert_eq!(v[0].path, "new.rs");
        assert!(
            v[0].status.starts_with('A'),
            "staged-new status should start with A, got {}",
            v[0].status
        );
    }

    #[test]
    fn uncommitted_unstaged_modification_is_caught() {
        let dir = init_repo();
        std::fs::write(dir.path().join("existing.txt"), "changed\n").unwrap();
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(
            v[0].status.contains('M'),
            "modified status should contain M, got {}",
            v[0].status
        );
    }

    #[test]
    fn uncommitted_staged_modification_is_caught() {
        let dir = init_repo();
        std::fs::write(dir.path().join("existing.txt"), "changed\n").unwrap();
        git(dir.path(), &["add", "existing.txt"]);
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.contains('M'));
    }

    #[test]
    fn uncommitted_deleted_tracked_file_is_caught() {
        let dir = init_repo();
        std::fs::remove_file(dir.path().join("existing.txt")).unwrap();
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.contains('D'));
    }

    #[test]
    fn uncommitted_renamed_tracked_file_is_caught() {
        let dir = init_repo();
        git(dir.path(), &["mv", "existing.txt", "renamed.txt"]);
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert!(
            v[0].status.contains('R'),
            "renamed status should contain R, got {}",
            v[0].status
        );
        // Porcelain prints "R  old -> new"; check_uncommitted_work
        // records the new path.
        assert_eq!(v[0].path, "renamed.txt");
    }

    // --- cas-bc1b: check_additive_only_branch_violations ------------------

    /// Helper: initialize a repo, create a `main` commit, branch off into
    /// `factory/worker`, and return the tempdir. The caller can then commit
    /// whatever it wants on `factory/worker` before running the check.
    fn init_branched_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("existing.txt"), "original\n").unwrap();
        git(p, &["add", "existing.txt"]);
        git(p, &["commit", "-q", "-m", "main: initial"]);
        git(p, &["checkout", "-q", "-b", "factory/worker"]);
        dir
    }

    #[test]
    fn branch_check_non_git_returns_empty() {
        let dir = tempdir().unwrap();
        assert!(
            check_additive_only_branch_violations(dir.path(), "main").is_empty()
        );
    }

    #[test]
    fn branch_check_missing_parent_branch_returns_empty() {
        // New repo with `main` but no such branch `nope` — merge-base
        // fails → empty. The gate must not fire when it can't reason
        // about history.
        let dir = init_branched_repo();
        let v = check_additive_only_branch_violations(dir.path(), "nope");
        assert!(v.is_empty(), "unknown parent must no-op, got: {v:?}");
    }

    #[test]
    fn branch_check_clean_branch_is_empty() {
        // factory/worker has the same HEAD as main → no commits → no
        // violations.
        let dir = init_branched_repo();
        let v = check_additive_only_branch_violations(dir.path(), "main");
        assert!(v.is_empty(), "branch with no commits must be clean: {v:?}");
    }

    #[test]
    fn branch_check_additive_commit_passes() {
        // cas-bc1b happy path: the worker committed one new file on
        // their branch. The branch-diff must be empty of M/D/R entries.
        let dir = init_branched_repo();
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: new.rs"]);
        let v = check_additive_only_branch_violations(dir.path(), "main");
        assert!(
            v.is_empty(),
            "purely additive branch commit must pass: {v:?}"
        );
    }

    #[test]
    fn branch_check_modifying_commit_fails() {
        // The worker modified an existing file on their branch. Must
        // be rejected.
        let dir = init_branched_repo();
        std::fs::write(dir.path().join("existing.txt"), "worker edit\n").unwrap();
        git(dir.path(), &["add", "existing.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: edit existing.txt"]);
        let v = check_additive_only_branch_violations(dir.path(), "main");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.starts_with('M'));
    }

    #[test]
    fn branch_check_deleting_commit_fails() {
        let dir = init_branched_repo();
        git(dir.path(), &["rm", "-q", "existing.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "chore: drop existing.txt"]);
        let v = check_additive_only_branch_violations(dir.path(), "main");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.starts_with('D'));
    }

    #[test]
    fn branch_check_ignores_main_worktree_drift() {
        // The core cas-4333 repro: main has a dirty uncommitted file
        // after the worker's branch forked. The branch-diff view must
        // not attribute that dirt to the worker. We achieve this by
        // comparing `main..HEAD` from inside the worker's worktree,
        // which only sees the worker's own commits.
        //
        // This test runs everything in a single tempdir because the
        // production fix uses `git -C <worker_worktree_path>` which
        // already gives us the CWD isolation we need; a separate
        // physical worktree is not necessary for the unit. The main-
        // drift scenario is that the worker's branch is additive *and*
        // there's uncommitted dirt in the tree that isn't on the
        // branch. The branch-diff must not report it.
        let dir = init_branched_repo();
        // Additive commit on the worker branch.
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: new.rs"]);
        // Now simulate drift: modify an existing tracked file but
        // leave it unstaged. The legacy `git diff HEAD` path would see
        // this and reject. The branch-diff path must not.
        std::fs::write(dir.path().join("existing.txt"), "drift\n").unwrap();
        let v = check_additive_only_branch_violations(dir.path(), "main");
        assert!(
            v.is_empty(),
            "uncommitted drift must not count against the branch: {v:?}"
        );
    }

    #[test]
    fn uncommitted_after_commit_is_empty() {
        // Complement scenario: the worker commits their work before
        // calling close. The gate must not fire.
        let dir = init_repo();
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "feat: add new.rs"],
        );
        let v = check_uncommitted_work(dir.path());
        assert!(
            v.is_empty(),
            "committed work must pass the gate: {v:?}"
        );
    }
}

#[cfg(test)]
mod code_review_gate_tests {
    //! Unit tests for the cas-b39f close gate helper. Covers the full
    //! decision matrix in [`run_code_review_gate`] under the option-(a)
    //! architecture where the worker passes findings in via
    //! `TaskCloseRequest.code_review_findings` before retrying close.
    //!
    //! The pure-Rust decision helper at
    //! `cas_store::code_review::close_gate::evaluate_gate` is already
    //! tested exhaustively in that module; these tests focus on the
    //! close-side glue — env role check, envelope plumbing, override
    //! path, docs-only skip, CODE_REVIEW_REQUIRED rejection.
    use super::*;
    use cas_types::{AutofixClass, Finding, FindingSeverity, Owner, ReviewOutcome};
    use tempfile::TempDir;

    fn base_task() -> Task {
        Task {
            id: "cas-test1".to_string(),
            title: "test".to_string(),
            status: TaskStatus::InProgress,
            ..Default::default()
        }
    }

    fn base_req(id: &str) -> TaskCloseRequest {
        TaskCloseRequest {
            id: id.to_string(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
        }
    }

    fn p0_finding() -> Finding {
        Finding {
            title: "SQL injection".to_string(),
            severity: FindingSeverity::P0,
            file: "src/auth.rs".to_string(),
            line: 42,
            why_it_matters: "allows login bypass".to_string(),
            autofix_class: AutofixClass::Manual,
            owner: Owner::Human,
            confidence: 0.95,
            evidence: vec!["format!(\"... {}\", user_input)".to_string()],
            pre_existing: false,
            suggested_fix: None,
            requires_verification: false,
        }
    }

    fn p2_finding() -> Finding {
        Finding {
            title: "dead import".to_string(),
            severity: FindingSeverity::P2,
            file: "src/lib.rs".to_string(),
            line: 3,
            why_it_matters: "minor".to_string(),
            autofix_class: AutofixClass::Manual,
            owner: Owner::ReviewFixer,
            confidence: 0.9,
            evidence: vec!["use foo::bar;".to_string()],
            pre_existing: false,
            suggested_fix: None,
            requires_verification: false,
        }
    }

    fn autofix_envelope(residual: Vec<Finding>) -> String {
        let env = ReviewOutcome {
            residual,
            pre_existing: Vec::new(),
            mode: "autofix".to_string(),
        };
        serde_json::to_string(&env).expect("serialize ReviewOutcome")
    }

    /// Build a throwaway git repo with one committed file, then stage
    /// whatever paths the caller names so `git diff --cached` sees
    /// them. Returns the tempdir so the caller controls its lifetime.
    fn repo_with_staged(paths: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        use std::process::Command;
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(p)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(&["add", "seed.txt"]);
        git(&["commit", "-q", "-m", "seed"]);
        for (path, contents) in paths {
            let full = p.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, contents).unwrap();
            git(&["add", path]);
        }
        dir
    }

    /// Serialize env-mutating tests so `CAS_AGENT_ROLE` changes don't
    /// leak between them.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    // --- Path classification ------------------------------------------------

    #[test]
    fn docs_and_tests_are_not_reviewable() {
        assert!(!is_reviewable_path("README.md"));
        assert!(!is_reviewable_path("docs/foo.txt"));
        assert!(!is_reviewable_path("crates/cas-store/tests/foo.rs"));
        assert!(!is_reviewable_path("src/foo_test.rs"));
        assert!(!is_reviewable_path("app/bar.test.tsx"));
        assert!(!is_reviewable_path("tests/integration.py"));
    }

    #[test]
    fn code_files_are_reviewable() {
        assert!(is_reviewable_path("src/main.rs"));
        assert!(is_reviewable_path("app/login.ts"));
        assert!(is_reviewable_path("pkg/server/handler.go"));
    }

    // --- run_code_review_gate branches --------------------------------------

    #[test]
    fn additive_only_task_bypasses_gate() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/evil.rs", "bad\n")]);
        let mut t = base_task();
        t.execution_note = Some("additive-only".to_string());
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(vec![p0_finding()]));
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    #[test]
    fn docs_only_diff_skips_gate_without_findings() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("README.md", "new content\n"), ("docs/x.md", "x\n")]);
        let t = base_task();
        let req = base_req(&t.id); // no findings
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "pure-docs diff must skip the review gate"
        );
    }

    #[test]
    fn code_change_without_findings_is_rejected_as_required() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path());
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("CODE_REVIEW_REQUIRED"));
                assert!(msg.contains("cas-code-review"));
                assert!(msg.contains("code_review_findings"));
            }
            other => panic!("expected CODE_REVIEW_REQUIRED reject, got {other:?}"),
        }
    }

    #[test]
    fn p0_residual_blocks_close() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(vec![p0_finding()]));
        let out = run_code_review_gate(&t, &req, dir.path());
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("P0 BLOCK"));
                assert!(msg.contains("SQL injection"));
                assert!(msg.contains("bypass_code_review=true"));
            }
            other => panic!("expected P0 block, got {other:?}"),
        }
    }

    #[test]
    fn p2_residual_does_not_block_close() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(vec![p2_finding()]));
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "P2 residual must route to Unit 8, not block close"
        );
    }

    #[test]
    fn empty_residual_with_envelope_allows_close() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn ok() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(Vec::new()));
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    #[test]
    fn malformed_envelope_validation_failure_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn ok() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        // Whitespace-only mode passes serde but fails validate().
        req.code_review_findings = Some(
            r#"{"residual":[],"pre_existing":[],"mode":"   "}"#.to_string(),
        );
        let out = run_code_review_gate(&t, &req, dir.path());
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("MALFORMED REVIEW ENVELOPE"));
            }
            other => panic!("expected malformed-envelope reject, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_envelope_json_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn ok() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some("not json at all".to_string());
        let out = run_code_review_gate(&t, &req, dir.path());
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("MALFORMED REVIEW ENVELOPE"));
                assert!(msg.contains("failed to parse"));
            }
            other => panic!("expected parse reject, got {other:?}"),
        }
    }

    #[test]
    fn supervisor_override_appends_decision_note() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let prev = std::env::var("CAS_AGENT_ROLE").ok();
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }

        let t = base_task();
        let mut req = base_req(&t.id);
        req.bypass_code_review = Some(true);
        req.reason = Some("P0 is a false positive, tracked in cas-xyz".to_string());

        let out = run_code_review_gate(&t, &req, dir.path());

        unsafe {
            match prev {
                Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
                None => std::env::remove_var("CAS_AGENT_ROLE"),
            }
        }

        match out {
            CodeReviewGateOutcome::AppendDecisionNote(note) => {
                assert!(note.contains("DECISION"));
                assert!(note.contains("supervisor"));
                assert!(note.contains("false positive"));
            }
            other => panic!("expected AppendDecisionNote, got {other:?}"),
        }
    }

    #[test]
    fn non_supervisor_override_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let prev = std::env::var("CAS_AGENT_ROLE").ok();
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "worker");
        }

        let t = base_task();
        let mut req = base_req(&t.id);
        req.bypass_code_review = Some(true);

        let out = run_code_review_gate(&t, &req, dir.path());

        unsafe {
            match prev {
                Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
                None => std::env::remove_var("CAS_AGENT_ROLE"),
            }
        }

        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("UNAUTHORIZED OVERRIDE"));
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn additive_only_plus_missing_findings_still_proceeds() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/evil.rs", "bad\n")]);
        let mut t = base_task();
        t.execution_note = Some("additive-only".to_string());
        let req = base_req(&t.id); // no findings, no override
        // additive-only short-circuits before the findings check.
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    #[test]
    fn non_git_project_root_skips_gate() {
        let _g = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let t = base_task();
        let req = base_req(&t.id);
        // Non-git dir → has_reviewable_changes returns false → skip.
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    // --- cas-3086: persisted-envelope fallback ------------------------------

    #[test]
    fn persisted_envelope_satisfies_gate_when_req_missing() {
        // Simulates supervisor-close: the worker persisted a clean
        // envelope on a prior (jailed) close attempt; supervisor
        // calls close without re-running review and without
        // bypass_code_review=true.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        t.deliverables.review_envelope = Some(autofix_envelope(Vec::new()));
        let req = base_req(&t.id); // no findings in request
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "persisted clean envelope must let supervisor-close proceed without bypass"
        );
    }

    #[test]
    fn persisted_envelope_with_p0_still_blocks() {
        // Forwarding a receipt does not weaken the P0 gate.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        t.deliverables.review_envelope = Some(autofix_envelope(vec![p0_finding()]));
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path());
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("P0 BLOCK"), "P0 must still block: {msg}");
            }
            other => panic!("expected P0 block on persisted envelope, got {other:?}"),
        }
    }

    #[test]
    fn request_envelope_takes_precedence_over_persisted() {
        // If the caller sends a fresh envelope, that's what the gate
        // sees — the persisted one is a fallback, not a merge.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        // Persisted envelope has a P0 — would block if chosen.
        t.deliverables.review_envelope = Some(autofix_envelope(vec![p0_finding()]));
        let mut req = base_req(&t.id);
        // Request envelope is clean — should let the close proceed.
        req.code_review_findings = Some(autofix_envelope(Vec::new()));
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "explicit request envelope must win over persisted fallback"
        );
    }

    #[test]
    fn persisted_malformed_envelope_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        t.deliverables.review_envelope = Some("not-json".to_string());
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path());
        assert!(
            matches!(out, CodeReviewGateOutcome::Reject(_)),
            "malformed persisted envelope must be rejected, not silently bypassed"
        );
    }

    // --- cas-fef4 + cas-3086: epic_subtask_receipts_are_clean ----------------

    /// Build a subtask carrying a specific review envelope (JSON string).
    fn subtask_with_envelope(id: &str, envelope: Option<String>) -> Task {
        let mut t = Task {
            id: id.to_string(),
            title: format!("subtask {id}"),
            status: TaskStatus::Closed,
            ..Default::default()
        };
        t.deliverables.review_envelope = envelope;
        t
    }

    fn envelope_with_pre_existing(
        residual: Vec<Finding>,
        pre_existing: Vec<Finding>,
    ) -> String {
        let env = ReviewOutcome {
            residual,
            pre_existing,
            mode: "autofix".to_string(),
        };
        serde_json::to_string(&env).expect("serialize ReviewOutcome")
    }

    #[test]
    fn epic_receipts_clean_when_all_subtasks_have_empty_envelopes() {
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some(autofix_envelope(Vec::new()))),
        ];
        assert!(
            epic_subtask_receipts_are_clean(&subtasks),
            "two clean subtask envelopes must cover the epic"
        );
    }

    #[test]
    fn epic_receipts_not_clean_when_no_subtasks() {
        // cas-3086: `_ => false` arm — an epic with zero subtasks has
        // nothing "covering" the union diff, so fall through to the
        // normal gate.
        assert!(!epic_subtask_receipts_are_clean(&[]));
    }

    #[test]
    fn epic_receipts_not_clean_when_subtask_has_residual_p0() {
        // cas-3086 defense-in-depth: a subtask envelope that somehow
        // leaked a residual P0 past its own close must NOT let the
        // epic bypass the gate.
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some(autofix_envelope(vec![p0_finding()]))),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "residual-P0 on any subtask must disqualify the bypass"
        );
    }

    #[test]
    fn epic_receipts_not_clean_when_subtask_has_pre_existing_p0() {
        // cas-fef4 (this task): a worker supplying an envelope of shape
        // `{ residual: [], pre_existing: [<real_p0>] }` satisfies the
        // old cas-3086 check (residual is clean) but smuggles a real
        // P0 past the epic-close gate by reclassifying it as
        // "pre-existing". The tightened clean-receipt semantics must
        // reject this forgery and fall through to run_code_review_gate
        // on the union diff.
        let forged = envelope_with_pre_existing(Vec::new(), vec![p0_finding()]);
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some(forged)),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "pre_existing-P0 smuggling must disqualify the bypass"
        );
    }

    #[test]
    fn epic_receipts_clean_when_pre_existing_is_only_subp0() {
        // Sanity check on the tightened check: non-P0 severities in
        // pre_existing (the normal case — legitimate low-severity
        // debt classified by the reviewer) must not block the bypass.
        let clean_with_low_pre = envelope_with_pre_existing(Vec::new(), vec![p2_finding()]);
        let subtasks = vec![subtask_with_envelope("s1", Some(clean_with_low_pre))];
        assert!(
            epic_subtask_receipts_are_clean(&subtasks),
            "pre_existing with only sub-P0 severities is legitimate and must not block bypass"
        );
    }

    #[test]
    fn epic_receipts_not_clean_when_subtask_envelope_missing_or_malformed() {
        // Missing envelope on any subtask → no structural proof → bypass declined.
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", None),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "missing envelope on any subtask must disqualify the bypass"
        );

        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some("not-json".to_string())),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "malformed envelope on any subtask must disqualify the bypass"
        );
    }
}

#[cfg(test)]
mod merge_state_gate_tests {
    //! Unit tests for the cas-95ce factory-branch merge-state close
    //! gate ([`run_factory_branch_merge_gate`]). The gate sits at
    //! `cas_task_close` line ~183, immediately after the existing
    //! [`check_unmerged_epic_branches`] guard for epic-type tasks, and
    //! BEFORE the cas-code-review gate / `bypass_code_review` plumbing.
    //!
    //! Why these tests are pure-helper instead of end-to-end
    //! `cas_task_close` calls:
    //!
    //! - The integration call site is mechanical (one
    //!   `pattern-match { Proceed => {} | Reject(msg) => return tool_error(msg) }`
    //!   block, mirroring the cas-code-review gate at line ~815).
    //! - Bypass-immunity is enforced **structurally**: the gate
    //!   function does not consume the bypass flag, and it runs at
    //!   the merge-state insertion (currently `cas_task_close`
    //!   ~line 184) — strictly upstream of the `bypass_code_review`
    //!   evaluation inside `run_code_review_gate`. The test sets
    //!   `req.bypass_code_review = Some(true)` and confirms the gate
    //!   still rejects, demonstrating bypass cannot reach this layer.
    //!
    //! Test layout mirrors `code_review_gate_tests` above.
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Build a repo with `main` carrying one seed commit, branch off
    /// into `factory/<worker>`, and return the tempdir on the worker
    /// branch. Caller adds whatever commits it wants on top.
    fn init_factory_repo(worker: &str) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", &format!("factory/{worker}")]);
        dir
    }

    fn worker_task(assignee: &str) -> Task {
        Task {
            id: "cas-test1".to_string(),
            title: "worker task".to_string(),
            status: TaskStatus::InProgress,
            assignee: Some(assignee.to_string()),
            ..Default::default()
        }
    }

    fn epic_task(assignee: Option<&str>) -> Task {
        Task {
            id: "cas-epic".to_string(),
            title: "the epic".to_string(),
            status: TaskStatus::InProgress,
            task_type: TaskType::Epic,
            assignee: assignee.map(str::to_string),
            ..Default::default()
        }
    }

    fn base_req(id: &str) -> TaskCloseRequest {
        TaskCloseRequest {
            id: id.to_string(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
        }
    }

    // --- The 6 named tests (per cas-95ce design / acceptance criteria) ----

    #[test]
    fn worker_task_close_rejects_when_factory_branch_unmerged() {
        // Worker committed two new files on factory/worker that never
        // landed on main. The gate must reject with stranded count + remediation.
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("a.rs"), "// a\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: a"]);
        std::fs::write(dir.path().join("b.rs"), "// b\n").unwrap();
        git(dir.path(), &["add", "b.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: b"]);

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());

        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(msg.contains("MERGE REQUIRED"), "missing header: {msg}");
                assert!(
                    msg.contains("factory/worker"),
                    "missing factory branch name: {msg}"
                );
                assert!(msg.contains("main"), "missing parent branch name: {msg}");
                assert!(
                    msg.contains("2 commit"),
                    "expected stranded count of 2 in message (anchored to 'commit' \
                     to avoid weak digit-anywhere match): {msg}"
                );
                assert!(
                    msg.contains("bypass_code_review=true"),
                    "remediation must call out bypass-immunity: {msg}"
                );
            }
            other => panic!("expected Reject for stranded factory branch, got {other:?}"),
        }
    }

    #[test]
    fn worker_task_close_succeeds_when_factory_branch_merged() {
        // factory/worker has no commits beyond main → 0 stranded → Proceed.
        let dir = init_factory_repo("worker");
        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "fully-merged factory branch must allow close, got {out:?}"
        );
    }

    #[test]
    fn worker_task_close_with_bypass_still_rejects_on_unmerged() {
        // Confirms `bypass_code_review=true` does NOT skip the
        // merge-state guard. Demonstrated at the type level — the
        // gate function does not consume the bypass flag — and at
        // the behavioral level by setting bypass=Some(true) on the
        // request and asserting the gate still rejects.
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("evil.rs"), "// stranded\n").unwrap();
        git(dir.path(), &["add", "evil.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: stranded"]);

        let task = worker_task("worker");
        let mut req = base_req(&task.id);
        req.bypass_code_review = Some(true);
        req.reason = Some("supervisor wants to skip review".to_string());

        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("bypass_code_review=true"),
                    "rejection message must spell out bypass-immunity policy: {msg}"
                );
            }
            other => panic!(
                "bypass_code_review must NOT skip merge-state guard, got {other:?}"
            ),
        }
    }

    #[test]
    fn worker_task_close_skipped_for_epic_type() {
        // The epic-close path is owned by check_unmerged_epic_branches
        // (line 161-182) which works at the epic-id branch namespace.
        // This per-task guard MUST NOT fire on epic-type tasks even
        // if their `assignee` field happens to be set (e.g.,
        // supervisor self-assigned an epic).
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("c.rs"), "// c\n").unwrap();
        git(dir.path(), &["add", "c.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: c"]);

        let task = epic_task(Some("worker"));
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "epic-type task must skip the per-task guard, got {out:?}"
        );
    }

    #[test]
    fn worker_task_close_skipped_for_no_assignee() {
        // Orphan tasks have no factory branch convention to resolve.
        // The guard must Proceed (covered by NoAssignee verification
        // skip elsewhere; here we just need it not to false-reject).
        let dir = init_factory_repo("worker");
        // Stranded commits exist on factory/worker, but our task has no assignee.
        std::fs::write(dir.path().join("d.rs"), "// d\n").unwrap();
        git(dir.path(), &["add", "d.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: d"]);

        let mut task = worker_task("worker");
        task.assignee = None;
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "no-assignee task must skip the guard, got {out:?}"
        );
    }

    #[test]
    fn worker_task_close_handles_missing_factory_branch() {
        // Worker convention is `factory/<assignee>`, but for a fresh
        // repo where no such branch ref exists, the guard must
        // Proceed (treat-as-merged) instead of false-rejecting. This
        // mirrors check_additive_only_branch_violations' graceful
        // degradation when git can't reason about history.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        // Note: no `factory/ghost` branch is created.

        let task = worker_task("ghost");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "missing factory branch must be treated as merged (graceful pass), got {out:?}"
        );
    }

    // --- Lower-level coverage on count_unmerged_factory_commits -------------

    #[test]
    fn count_returns_zero_for_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            count_unmerged_factory_commits(dir.path(), "factory/x", "main"),
            0,
            "non-git dir must degrade to 0"
        );
    }

    #[test]
    fn count_matches_committed_delta() {
        let dir = init_factory_repo("worker");
        // 3 commits on factory/worker beyond main.
        for (i, name) in ["x.rs", "y.rs", "z.rs"].iter().enumerate() {
            std::fs::write(dir.path().join(name), format!("// {i}\n")).unwrap();
            git(dir.path(), &["add", name]);
            git(dir.path(), &["commit", "-q", "-m", &format!("feat: {name}")]);
        }
        assert_eq!(
            count_unmerged_factory_commits(dir.path(), "factory/worker", "main"),
            3,
            "count must equal commits on factory/worker beyond main"
        );
    }
}

#[cfg(test)]
mod epic_status_gate_tests {
    //! cas-8f8f: per-child branch merge-state report + epic-close gate.
    //!
    //! Layered on top of the cas-95ce per-task gate. The report
    //! rendering is a pure function of `Vec<EpicChildBranchStatus>`,
    //! and the gate is a thin filter on top of `collect_epic_branch_statuses`
    //! that rejects when any child has stranded factory commits.
    //! Bypass-immunity is structural (gate signature does not consume
    //! the bypass flag), and `run_epic_close_merge_gate` is also
    //! upstream of the cas-code-review bypass evaluation in
    //! [`run_code_review_gate`] — same shape as cas-95ce.
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Set up a tempdir git repo where `main` is the seed and each
    /// of `workers` has a `factory/<name>` branch with `commits_per`
    /// additive commits beyond `main`. Returns the tempdir handle.
    fn init_epic_repo(workers_with_strands: &[(&str, usize)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        for (worker, n) in workers_with_strands {
            git(p, &["checkout", "-q", "-b", &format!("factory/{worker}")]);
            for i in 0..*n {
                let fname = format!("{worker}-{i}.rs");
                std::fs::write(p.join(&fname), format!("// {worker} {i}\n")).unwrap();
                git(p, &["add", &fname]);
                git(p, &["commit", "-q", "-m", &format!("feat: {fname}")]);
            }
            git(p, &["checkout", "-q", "main"]);
        }
        dir
    }

    fn child(id: &str, status: TaskStatus, assignee: Option<&str>) -> Task {
        Task {
            id: id.to_string(),
            title: format!("child {id}"),
            status,
            assignee: assignee.map(str::to_string),
            ..Default::default()
        }
    }

    fn epic(id: &str) -> Task {
        Task {
            id: id.to_string(),
            title: format!("epic {id}"),
            status: TaskStatus::InProgress,
            task_type: TaskType::Epic,
            ..Default::default()
        }
    }

    fn base_req(id: &str) -> TaskCloseRequest {
        TaskCloseRequest {
            id: id.to_string(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
        }
    }

    // --- collect_epic_branch_statuses ---------------------------------------

    #[test]
    fn factory_epic_status_returns_clean_report_when_all_merged() {
        // All workers fully merged → all children show 0 unmerged.
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 0)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::Closed, Some("bravo")),
        ];
        let statuses = collect_epic_branch_statuses(&subtasks, "main", dir.path());
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses.iter().all(|s| s.unmerged_count == 0),
            "all children should report 0 unmerged: {statuses:?}"
        );
        assert!(
            statuses.iter().all(|s| s.factory_branch.is_some()),
            "every child has an assignee → every row has a factory branch"
        );

        let report = render_epic_status_report("cas-epic", "main", &statuses);
        assert!(report.contains("Epic cas-epic"));
        assert!(report.contains("factory/alpha"));
        assert!(report.contains("factory/bravo"));
        assert!(
            report.contains("All child factory branches are merged"),
            "clean report must include the all-merged confirmation: {report}"
        );
    }

    #[test]
    fn factory_epic_status_reports_unmerged_per_worker() {
        // Two of three workers carry stranded commits; alpha is clean.
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 2), ("charlie", 5)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::Closed, Some("bravo")),
            child("cas-c3", TaskStatus::Closed, Some("charlie")),
        ];
        let statuses = collect_epic_branch_statuses(&subtasks, "main", dir.path());
        assert_eq!(statuses.len(), 3);

        let by_id: std::collections::HashMap<_, _> =
            statuses.iter().map(|s| (s.task_id.as_str(), s)).collect();
        assert_eq!(by_id["cas-c1"].unmerged_count, 0, "alpha is clean");
        assert_eq!(by_id["cas-c2"].unmerged_count, 2, "bravo has 2 stranded");
        assert_eq!(by_id["cas-c3"].unmerged_count, 5, "charlie has 5 stranded");

        // Each row with stranded commits must carry a non-None last_commit_unix
        // (the branch exists locally and has at least one commit).
        assert!(by_id["cas-c2"].last_commit_unix.is_some());
        assert!(by_id["cas-c3"].last_commit_unix.is_some());

        let report = render_epic_status_report("cas-epic", "main", &statuses);
        assert!(
            report.contains("2 child task(s) carry stranded factory commits"),
            "report must summarize stranded count = 2 (bravo + charlie): {report}"
        );
    }

    #[test]
    fn factory_epic_status_handles_no_subtasks() {
        // Epic with zero children produces a "no child tasks" report.
        let dir = init_epic_repo(&[]);
        let statuses = collect_epic_branch_statuses(&[], "main", dir.path());
        assert!(statuses.is_empty());

        let report = render_epic_status_report("cas-epic-empty", "main", &statuses);
        assert!(
            report.contains("(no child tasks)"),
            "empty-subtasks report must emit the explicit no-children marker: {report}"
        );
    }

    #[test]
    fn factory_epic_status_includes_assigneeless_children() {
        // Children without an assignee are reported with em-dash placeholders
        // for branch / count so the report is complete; the gate filters
        // them out separately.
        let dir = init_epic_repo(&[]);
        let subtasks = vec![child("cas-orphan", TaskStatus::InProgress, None)];
        let statuses = collect_epic_branch_statuses(&subtasks, "main", dir.path());
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].factory_branch.is_none());
        assert_eq!(statuses[0].unmerged_count, 0);

        let report = render_epic_status_report("cas-epic", "main", &statuses);
        assert!(report.contains("cas-orphan"));
        assert!(
            report.contains("| — | — |"),
            "assigneeless rows must use em-dash for branch + unmerged columns: {report}"
        );
    }

    // --- run_epic_close_merge_gate ------------------------------------------

    #[test]
    fn epic_close_rejects_when_any_child_factory_unmerged() {
        // 3 children, 1 has stranded commits → gate Rejects with detail.
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 3)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::InProgress, Some("bravo")),
        ];
        let task = epic("cas-754b-test");
        let req = base_req(&task.id);

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        match out {
            EpicCloseGateOutcome::Reject(msg) => {
                assert!(msg.contains("MERGE REQUIRED"), "missing header: {msg}");
                assert!(msg.contains("cas-754b-test"), "missing epic id: {msg}");
                assert!(msg.contains("cas-c2"), "missing offending child id: {msg}");
                assert!(msg.contains("factory/bravo"), "missing branch: {msg}");
                assert!(msg.contains("3 commit"), "missing stranded count: {msg}");
                assert!(
                    !msg.contains("cas-c1"),
                    "must not list clean children in the rejection: {msg}"
                );
                assert!(
                    msg.contains("bypass_code_review=true"),
                    "rejection must call out bypass-immunity: {msg}"
                );
                assert!(
                    msg.contains("epic_status"),
                    "rejection must point at the diagnostic action: {msg}"
                );
            }
            other => panic!(
                "expected Reject for stranded child branch, got {other:?}"
            ),
        }
    }

    #[test]
    fn epic_close_succeeds_when_all_children_merged() {
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 0), ("charlie", 0)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::Closed, Some("bravo")),
            child("cas-c3", TaskStatus::Closed, Some("charlie")),
        ];
        let task = epic("cas-epic-clean");
        let req = base_req(&task.id);

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        assert!(
            matches!(out, EpicCloseGateOutcome::Proceed),
            "all-merged epic must allow close, got {out:?}"
        );
    }

    #[test]
    fn epic_close_with_bypass_still_rejects_on_unmerged_child() {
        // Bypass-immunity at the structural level — gate has no
        // bypass parameter — and behavioral level — even with
        // bypass=Some(true) on the request, the gate rejects.
        let dir = init_epic_repo(&[("alpha", 1)]);
        let subtasks = vec![child("cas-c1", TaskStatus::InProgress, Some("alpha"))];
        let task = epic("cas-epic-bypass");
        let mut req = base_req(&task.id);
        req.bypass_code_review = Some(true);
        req.reason = Some("supervisor wants to skip review".to_string());

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        match out {
            EpicCloseGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("bypass_code_review=true"),
                    "rejection must spell out bypass-immunity policy: {msg}"
                );
            }
            other => panic!(
                "bypass_code_review must NOT skip the epic merge gate, got {other:?}"
            ),
        }
    }

    #[test]
    fn epic_close_gate_skips_non_epic_tasks() {
        // Symmetrical to cas-95ce's per-task gate: this one only fires
        // on Epic-type tasks.
        let dir = init_epic_repo(&[("alpha", 5)]);
        let subtasks = vec![child("cas-c1", TaskStatus::InProgress, Some("alpha"))];
        let task = child("cas-not-epic", TaskStatus::InProgress, None); // non-epic
        let req = base_req(&task.id);

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        assert!(
            matches!(out, EpicCloseGateOutcome::Proceed),
            "non-epic task must skip this gate, got {out:?}"
        );
    }

    // --- snapshot test on report shape --------------------------------------

    #[test]
    fn epic_status_report_snapshot_shape_is_stable() {
        // Pin the exact report layout. Future contributors changing
        // the markdown structure must update this assertion deliberately.
        let statuses = vec![
            EpicChildBranchStatus {
                task_id: "cas-aaaa".to_string(),
                task_status: TaskStatus::Closed,
                assignee: Some("alpha".to_string()),
                factory_branch: Some("factory/alpha".to_string()),
                unmerged_count: 0,
                last_commit_unix: Some(1735689600), // 2025-01-01 00:00 UTC
            },
            EpicChildBranchStatus {
                task_id: "cas-bbbb".to_string(),
                task_status: TaskStatus::InProgress,
                assignee: Some("bravo".to_string()),
                factory_branch: Some("factory/bravo".to_string()),
                unmerged_count: 2,
                last_commit_unix: Some(1735776000), // 2025-01-02 00:00 UTC
            },
            EpicChildBranchStatus {
                task_id: "cas-cccc".to_string(),
                task_status: TaskStatus::InProgress,
                assignee: None,
                factory_branch: None,
                unmerged_count: 0,
                last_commit_unix: None,
            },
        ];
        let report = render_epic_status_report("cas-754b", "epic/foo", &statuses);

        // Status column uses TaskStatus's Display impl (snake_case:
        // closed, in_progress) per round-1 cas-code-review fix —
        // matches the rest of the CLI's status rendering.
        let expected = "\
Epic cas-754b — factory branch status\n\
Parent branch: epic/foo\n\
\n\
| Task | Status | Assignee | Factory branch | Unmerged | Last commit |\n\
|------|--------|----------|----------------|----------|-------------|\n\
| cas-aaaa | closed | alpha | factory/alpha | 0 | 2025-01-01 00:00 UTC |\n\
| cas-bbbb | in_progress | bravo | factory/bravo | 2 | 2025-01-02 00:00 UTC |\n\
| cas-cccc | in_progress | — | — | — | — |\n\
\n\
⚠️  1 child task(s) carry stranded factory commits. \
Epic close will be hard-blocked until they are merged.\n";

        assert_eq!(
            report, expected,
            "report shape regressed; review and update if intentional"
        );
    }

    // --- Lower-level helpers ------------------------------------------------

    #[test]
    fn format_unix_timestamp_is_iso_utc() {
        // 1735689600 = 2025-01-01T00:00:00Z
        assert_eq!(format_unix_timestamp(1735689600), "2025-01-01 00:00 UTC");
    }

    #[test]
    fn last_commit_unix_returns_none_for_missing_branch() {
        let dir = init_epic_repo(&[]);
        assert_eq!(last_commit_unix(dir.path(), "factory/ghost"), None);
    }

    #[test]
    fn last_commit_unix_returns_some_for_existing_branch() {
        let dir = init_epic_repo(&[("alpha", 1)]);
        let ts = last_commit_unix(dir.path(), "factory/alpha");
        assert!(ts.is_some(), "branch with commits must yield Some(ts)");
        assert!(ts.unwrap() > 0);
    }
}
