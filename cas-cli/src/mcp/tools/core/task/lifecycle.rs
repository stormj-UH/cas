use crate::harness_policy::is_worker_without_subagents_from_env;
use crate::mcp::tools::core::imports::*;

pub(crate) mod close_ops;

impl CasCore {
    pub async fn cas_task_create(
        &self,
        Parameters(req): Parameters<TaskCreateRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let id = task_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let task_type: TaskType = req.task_type.parse().unwrap_or(TaskType::Task);
        let labels: Vec<String> = req
            .labels
            .map(|l| {
                l.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let status = TaskStatus::Open;
        let blocked_by_ids: Vec<String> = req
            .blocked_by
            .as_deref()
            .map(|blocked_by| {
                blocked_by
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let epic_id = req
            .epic
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        let execution_note = crate::mcp::tools::types::validate_execution_note(
            req.execution_note.as_deref(),
        )
        .map_err(|msg| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(msg),
            data: None,
        })?;

        let now = chrono::Utc::now();
        let task = Task {
            id: id.clone(),
            scope: crate::types::Scope::Project, // MCP tasks are project-scoped
            title: req.title,
            description: req.description.unwrap_or_default(),
            design: req.design.unwrap_or_default(),
            acceptance_criteria: req.acceptance_criteria.unwrap_or_default(),
            demo_statement: req.demo_statement.unwrap_or_default(),
            execution_note,
            notes: req.notes.unwrap_or_default(),
            status,
            priority: Priority(req.priority.min(4) as i32),
            task_type,
            assignee: req.assignee,
            labels,
            created_at: now,
            updated_at: now,
            closed_at: None,
            close_reason: None,
            external_ref: req.external_ref,
            content_hash: None,
            branch: None,
            deliverables: crate::types::TaskDeliverables::default(),
            team_id: None,
            worktree_id: None,
            pending_verification: false,
            pending_worktree_merge: false,
            epic_verification_owner: None,
            share: None,
        };

        task_store
            .create_atomic(&task, &blocked_by_ids, epic_id.as_deref(), Some("mcp"))
            .map_err(|e| {
                let is_invalid_epic = match &e {
                    cas_store::StoreError::TaskNotFound(missing_id) => {
                        epic_id.as_deref() == Some(missing_id.as_str())
                    }
                    cas_store::StoreError::Parse(msg) => {
                        msg.starts_with("Task ") && msg.contains(" is not an epic")
                    }
                    _ => false,
                };
                let (code, message) = if is_invalid_epic {
                    let msg = match &e {
                        cas_store::StoreError::TaskNotFound(missing_id) => {
                            format!("Epic not found: {missing_id}")
                        }
                        _ => e.to_string(),
                    };
                    (ErrorCode::INVALID_PARAMS, msg)
                } else {
                    (
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to create task: {e}"),
                    )
                };
                McpError {
                    code,
                    message: Cow::from(message),
                    data: None,
                }
            })?;

        if let Ok(search) = self.open_search_index() {
            let _ = search.index_task(&task);
        }

        // Auto-create epic branch for all epic creates (regardless of start flag)
        // Epics get a branch (not a worktree) - workers get worktrees when spawned
        let branch_info = if task.task_type == crate::types::TaskType::Epic
            && task.branch.as_deref().unwrap_or("").is_empty()
        {
            use crate::worktree::GitOperations;

            // Get project root (parent of .cas directory)
            let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);

            // Try to create epic branch using git operations directly
            // This works regardless of whether worktrees are enabled
            if GitOperations::is_git_available() {
                if let Ok(git_ops) =
                    GitOperations::detect_repo_root(project_root).map(GitOperations::new)
                {
                    let branch_name = format!("epic/{}-{}", slugify_for_branch(&task.title), id);
                    match git_ops.create_branch_if_not_exists(&branch_name) {
                        Ok(created) => {
                            // Update epic with branch info (no worktree)
                            let task_store = self.open_task_store()?;
                            if let Ok(mut updated_task) = task_store.get(&id) {
                                if updated_task.branch.as_deref().unwrap_or("").is_empty() {
                                    updated_task.branch = Some(branch_name.clone());
                                }
                                let _ = task_store.update(&updated_task);
                            }

                            // Push to origin only when explicitly enabled
                            let push_enabled = std::env::var("CAS_PUSH_EPIC_BRANCH")
                                .map(|v| {
                                    let v = v.to_ascii_lowercase();
                                    v == "1" || v == "true" || v == "on"
                                })
                                .unwrap_or(false);
                            if created && push_enabled {
                                if let Err(e) = git_ops.push_branch(&branch_name) {
                                    eprintln!(
                                        "[CAS] Warning: Failed to push epic branch to origin: {e}"
                                    );
                                }
                            }

                            Some(format!(
                                "\n\n🌿 Epic branch created: {branch_name}\n   Workers will branch from this when spawned."
                            ))
                        }
                        Err(e) => {
                            // Log but continue - branch creation is optional enhancement
                            eprintln!("Warning: Failed to create epic branch: {e}");
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self::success(format!(
            "Created task: {} - {} (P{}){}",
            id,
            task.title,
            task.priority.0,
            branch_info.unwrap_or_default()
        )))
    }

    /// List ready tasks
    pub async fn cas_task_ready(
        &self,
        Parameters(req): Parameters<TaskReadyBlockedRequest>,
    ) -> Result<CallToolResult, McpError> {
        use cas_types::TaskSortOptions;

        let task_store = self.open_task_store()?;

        // If epic filter specified, get subtasks and filter to ready ones
        let mut tasks = if let Some(ref epic_id) = req.epic {
            let subtasks = task_store.get_subtasks(epic_id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get subtasks for epic {epic_id}: {e}")),
                data: None,
            })?;
            // Filter to only ready tasks (open, not blocked)
            subtasks
                .into_iter()
                .filter(|t| {
                    t.status == cas_types::TaskStatus::Open
                        && task_store
                            .get_blockers(&t.id)
                            .map_or(true, |b| b.is_empty())
                })
                .collect()
        } else {
            task_store.list_ready().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list: {e}")),
                data: None,
            })?
        };

        // Apply sorting
        let sort_opts =
            TaskSortOptions::from_params(req.sort.as_deref(), req.sort_order.as_deref());
        sort_tasks(&mut tasks, &sort_opts);

        if tasks.is_empty() {
            let msg = if req.epic.is_some() {
                "No ready tasks in this epic"
            } else {
                "No ready tasks"
            };
            return Ok(Self::success(msg));
        }

        let limit = req.limit.unwrap_or(10);
        let mut output = format!("Ready tasks ({}):\n\n", tasks.len().min(limit));
        for task in tasks.iter().take(limit) {
            output.push_str(&format!(
                "- [{}] P{} {} - {}\n",
                task.id, task.priority.0, task.task_type, task.title
            ));
        }

        Ok(Self::success(output))
    }

    /// Start a task
    pub async fn cas_task_start(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if task.status == TaskStatus::Closed {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Cannot start a closed task. Use reopen first.",
            ));
        }

        // Auto-claim the task with a lease
        let agent_id = self.get_agent_id()?;

        // Check if agent has pending verification (blocks starting new tasks)
        if let Some((blocked_task_id, blocked_task_title)) =
            self.check_pending_verification(&agent_id)?
        {
            // Allow if starting the same task that's blocking (resuming work)
            if blocked_task_id != req.id {
                let is_worker_without_subagents = is_worker_without_subagents_from_env();

                return Ok(Self::tool_error(format!(
                    "🚫 VERIFICATION PENDING\n\n\
                    You have an unverified task: [{}] {}\n\n\
                    Before starting new work, complete verification:\n\
                    {}\n\n\
                    Use `cas_task_show` with id={} to see task details.",
                    blocked_task_id,
                    blocked_task_title,
                    if is_worker_without_subagents {
                        format!(
                            "1. Ask supervisor to verify task {blocked_task_id} (task-verifier or direct mcp__cs__verification)\n\
                            2. Fix any issues found\n\
                            3. Ask supervisor to close the task once verification is approved"
                        )
                    } else {
                        format!(
                            "1. Spawn the 'task-verifier' agent with task_id={blocked_task_id}\n\
                            2. Fix any issues found\n\
                            3. Once verified, close the task and start new work"
                        )
                    },
                    blocked_task_id
                )));
            }
        }
        let agent_store = self.open_agent_store()?;

        // Check agent role for supervisor/worker-specific logic
        let is_worker = agent_store
            .get(&agent_id)
            .map(|a| a.role == cas_types::AgentRole::Worker)
            .unwrap_or(false);

        // Check if supervisor is trying to start a non-epic task
        if let Ok(agent) = agent_store.get(&agent_id) {
            if agent.role == cas_types::AgentRole::Supervisor
                && task.task_type != crate::types::TaskType::Epic
            {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Supervisors cannot start non-epic tasks. To delegate work:\n\n\
                        1. Assign to existing worker:\n\
                           mcp__cas__task action=update id=<task_id> assignee=<worker_name>\n\
                           mcp__cas__coordination action=message target=<worker_name> message=\"Task <task_id> assigned\"\n\n\
                        2. Or spawn a new worker:\n\
                           mcp__cas__coordination action=spawn_workers count=1\n\n\
                        Supervisors coordinate and review; workers execute tasks.",
                    ),
                    data: None,
                });
            }
        }

        let config = self.load_config();
        let lease_duration = (config.lease().default_duration_mins as i64) * 60;

        let claim_info =
            match agent_store.try_claim(&req.id, &agent_id, lease_duration, Some("Task started")) {
                Ok(ClaimResult::Success(lease)) => Some(format!(
                    " (claimed until {})",
                    lease.expires_at.format("%H:%M")
                )),
                Ok(ClaimResult::AlreadyClaimed {
                    held_by,
                    expires_at,
                    ..
                }) => {
                    if held_by == agent_id {
                        Some(format!(
                            " (already claimed by you until {})",
                            expires_at.format("%H:%M")
                        ))
                    } else {
                        return Err(Self::error(
                            ErrorCode::INVALID_PARAMS,
                            format!(
                                "Task is locked by agent {} until {}",
                                held_by,
                                expires_at.format("%H:%M")
                            ),
                        ));
                    }
                }
                Ok(_) => None, // TaskNotFound, NotClaimable, Unauthorized - log but continue
                Err(e) => {
                    // Log but continue - claim is optional enhancement
                    eprintln!("Warning: Failed to claim task: {e}");
                    None
                }
            };

        // Record working epic if this task belongs to one
        // This is used by the exit blocker to ensure all epic subtasks are completed
        // Also look up the parent epic's worktree and sibling notes
        let mut parent_worktree_info: Option<String> = None;
        let mut epic_ownership_info: Option<String> = None;
        let mut sibling_notes_info: Option<String> = None;
        if let Ok(deps) = task_store.get_dependencies(&req.id) {
            for dep in deps {
                if dep.dep_type == crate::types::DependencyType::ParentChild {
                    // This task is a subtask - dep.to_id is the parent epic
                    if let Ok(parent) = task_store.get(&dep.to_id) {
                        if parent.task_type == crate::types::TaskType::Epic {
                            let _ = agent_store.add_working_epic(&agent_id, &parent.id);

                            // Fetch sibling task notes for epic context
                            if let Ok(sibling_notes) =
                                task_store.get_sibling_notes(&parent.id, &req.id)
                            {
                                if !sibling_notes.is_empty() {
                                    let mut notes_output = String::from(
                                        "\n\n📋 SIBLING TASK NOTES (from other workers on this epic):",
                                    );
                                    for (task_id, title, notes) in sibling_notes {
                                        notes_output.push_str(&format!(
                                            "\n\n**[{task_id}] {title}**\n{notes}"
                                        ));
                                    }
                                    sibling_notes_info = Some(notes_output);
                                }
                            }

                            // Epic ownership logic - only for supervisors, not workers
                            // Workers just execute their assigned tasks; supervisors own the epic
                            if !is_worker {
                                // Auto-start epic if not already in progress
                                let epic_was_started = if parent.status != TaskStatus::InProgress {
                                    let mut parent_mut = parent.clone();
                                    parent_mut.status = TaskStatus::InProgress;
                                    parent_mut.updated_at = chrono::Utc::now();
                                    task_store.update(&parent_mut).is_ok()
                                } else {
                                    false
                                };

                                // Claim epic for this agent (they now own the entire epic)
                                let epic_claim_status = match agent_store.try_claim(
                                    &parent.id,
                                    &agent_id,
                                    lease_duration,
                                    Some("Epic auto-claimed from subtask start"),
                                ) {
                                    Ok(ClaimResult::Success(lease)) => {
                                        format!(
                                            "claimed until {}",
                                            lease.expires_at.format("%H:%M")
                                        )
                                    }
                                    Ok(ClaimResult::AlreadyClaimed {
                                        held_by,
                                        expires_at,
                                        ..
                                    }) => {
                                        if held_by == agent_id {
                                            format!(
                                                "already yours until {}",
                                                expires_at.format("%H:%M")
                                            )
                                        } else {
                                            format!("held by {held_by}")
                                        }
                                    }
                                    _ => "unclaimed".to_string(),
                                };

                                // Get subtask count
                                let subtask_count = task_store
                                    .get_subtasks(&parent.id)
                                    .map(|s| s.len())
                                    .unwrap_or(0);

                                // Build epic ownership message
                                let started_note = if epic_was_started {
                                    " (auto-started)"
                                } else {
                                    ""
                                };
                                epic_ownership_info = Some(format!(
                                    "\n\n📋 EPIC OWNERSHIP: You are now responsible for epic [{}] {}{}\n   Subtasks: {} total | Status: {}",
                                    parent.id,
                                    parent.title,
                                    started_note,
                                    subtask_count,
                                    epic_claim_status
                                ));
                            }

                            // Look up the parent epic's worktree
                            if let Some(ref worktree_id) = parent.worktree_id {
                                if let Ok(wt_store) = self.open_worktree_store() {
                                    if let Ok(worktree) = wt_store.get(worktree_id) {
                                        if worktree.path.exists() {
                                            parent_worktree_info = Some(format!(
                                                "\n\n🌳 This task belongs to epic [{}] {}\n   Work in directory: {}\n   Branch: {}",
                                                parent.id,
                                                parent.title,
                                                worktree.path.display(),
                                                worktree.branch
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Try to create worktree if enabled AND task is an epic
        // Worktrees are scoped to epics, not individual tasks
        let worktree_info = if task.task_type == crate::types::TaskType::Epic {
            // Track this epic in working_epics for exit blocker
            // This ensures we can't stop while epic subtasks remain incomplete
            let _ = agent_store.add_working_epic(&agent_id, &req.id);

            if let Some(manager) = self.worktree_manager() {
                match manager.create_for_epic(&req.id, Some(&agent_id)) {
                    Ok(worktree) => {
                        // Store the worktree record
                        if let Ok(wt_store) = self.open_worktree_store() {
                            let _ = wt_store.add(&worktree);
                        }

                        // Update epic with worktree info
                        task.branch = Some(worktree.branch.clone());
                        task.worktree_id = Some(worktree.id.clone());

                        Some(format!(
                            "\n\n🌳 Worktree created for isolated development:\n   Branch: {}\n   Path: {}\n\n⚠️  Work in this directory for all changes.",
                            worktree.branch,
                            worktree.path.display()
                        ))
                    }
                    Err(e) => {
                        // Log but continue - worktree is optional enhancement
                        eprintln!("Warning: Failed to create worktree: {e}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        task.status = TaskStatus::InProgress;
        task.updated_at = chrono::Utc::now();

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // For subtasks, show parent epic's worktree; for epics, show newly created worktree
        let wt_info = parent_worktree_info.or(worktree_info).unwrap_or_default();

        Ok(Self::success(format!(
            "Started task: {} - {}{}{}{}{}{}",
            req.id,
            task.title,
            claim_info.unwrap_or_default(),
            epic_ownership_info.unwrap_or_default(),
            wt_info,
            sibling_notes_info.unwrap_or_default(),
            Self::workflow_guidance()
        )))
    }
}
