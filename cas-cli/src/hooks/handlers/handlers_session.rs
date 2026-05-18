use crate::hooks::handlers::*;

pub fn handle_session_start(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let timer = TraceTimer::new();

    // Record session start for analytics and register agent
    if let Some(cas_root) = cas_root {
        let mut stores = HookStores::new(cas_root);

        if let Some(sqlite_store) = stores.sqlite() {
            let session = Session::new(
                input.session_id.clone(),
                input.cwd.clone(),
                input.permission_mode.clone(),
            );
            if sqlite_store.start_session(&session).is_ok() {
                eprintln!(
                    "cas: Session {} started",
                    &input.session_id[..8.min(input.session_id.len())]
                );
            }
        }

        // Notify daemon via socket for instant agent registration
        // Daemon tracks PID → session mapping in memory (no files needed)
        // Pass agent_name and agent_role from this process's env (set by factory mode)
        use crate::agent_id::get_cc_pid_for_hook;
        let cc_pid = get_cc_pid_for_hook();
        let agent_name = std::env::var("CAS_AGENT_NAME").ok();
        let agent_role = std::env::var("CAS_AGENT_ROLE").ok();
        let clone_path = std::env::var("CAS_CLONE_PATH").ok();

        // Helper to register agent directly in database
        let register_directly = |stores: &mut HookStores| {
            if let Some(agent_store) = stores.agents() {
                use crate::orchestration::names as friendly_names;
                use crate::types::{Agent, AgentRole};

                let name = agent_name.clone().unwrap_or_else(friendly_names::generate);
                let mut agent = Agent::new(input.session_id.clone(), name);
                agent.pid = Some(cc_pid);
                // PID-reuse fingerprint (cas-ea46 / cas-389c): pair agent.pid
                // with the /proc/<pid>/stat starttime so the heartbeat liveness
                // gate can detect kernel PID recycling. Previously missing on
                // this fallback path — caught by the source-scanning test in
                // mcp::daemon_tests.
                #[cfg(feature = "mcp-server")]
                crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, cc_pid);
                agent.machine_id = Some(Agent::get_or_generate_machine_id());

                // Set role from environment
                if let Some(ref role_str) = agent_role {
                    if let Ok(role) = role_str.parse::<AgentRole>() {
                        agent.role = role;
                    }
                }

                // Store clone path in metadata for factory workers
                if let Some(ref path) = clone_path {
                    agent
                        .metadata
                        .insert("clone_path".to_string(), path.clone());
                }

                if let Err(reg_err) = agent_store.register(&agent) {
                    eprintln!("cas: Failed to register agent: {reg_err}");
                } else {
                    eprintln!(
                        "cas: Registered agent directly (pid: {cc_pid}, role: {agent_role:?})"
                    );
                }
            }
        };

        #[cfg(feature = "mcp-server")]
        {
            use crate::mcp::socket::{DaemonEvent, send_event};
            let event = DaemonEvent::SessionStart {
                session_id: input.session_id.clone(),
                agent_name: agent_name.clone(),
                agent_role: agent_role.clone(),
                cc_pid,
                clone_path: clone_path.clone(),
            };
            match send_event(cas_root, &event) {
                Ok(_) => eprintln!(
                    "cas: Notified daemon of session start (pid: {}, role: {:?})",
                    cc_pid,
                    std::env::var("CAS_AGENT_ROLE").ok()
                ),
                Err(e) => {
                    // Daemon socket not available - register directly in database as fallback
                    eprintln!("cas: Daemon not available ({e}), registering directly");
                    register_directly(&mut stores);
                }
            }
        }

        #[cfg(not(feature = "mcp-server"))]
        {
            // Without MCP server, register directly
            register_directly(&mut stores);
        }

        // Write OTEL context for telemetry correlation
        let project_id = crate::cloud::get_project_canonical_id();
        let project_path = cas_root.parent().map(|p| p.to_string_lossy().to_string());

        // Check for active task (reuses cached task store)
        let active_task_id = stores
            .tasks()
            .and_then(|ts| {
                ts.list(Some(TaskStatus::InProgress))
                    .ok()
                    .and_then(|tasks| tasks.first().map(|t| t.id.clone()))
            });

        let otel_ctx = OtelContext::new(input.session_id.clone())
            .with_project_id(project_id)
            .with_project_path(project_path)
            .with_permission_mode(input.permission_mode.clone())
            .with_task_id(active_task_id);

        if let Err(e) = otel_ctx.write(cas_root) {
            eprintln!("cas: Warning: Failed to write OTEL context: {e}");
        }

        // Cleanup orphaned tasks from crashed/interrupted previous sessions
        let reopened = cleanup_orphaned_tasks(cas_root);
        if reopened > 0 {
            eprintln!("cas: Reopened {reopened} orphaned task(s) from previous session");
        }
    }

    // Check if we're in plan mode
    let is_plan_mode = input.permission_mode.as_deref() == Some("plan");

    // Load config to check AI context setting
    let config = cas_root
        .map(|r| Config::load(r).unwrap_or_default())
        .unwrap_or_default();

    // Need cas_root for context building
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // Build appropriate context based on mode
    let context = if is_plan_mode {
        eprintln!("cas: Plan mode detected, building planning context");
        build_plan_context(input, 10, cas_root)?
    } else if config.hooks.as_ref().map(|h| h.ai_context).unwrap_or(false) {
        // Try AI-powered context selection
        eprintln!("cas: Using AI-assisted context prioritization");
        match build_context_ai(input, 5, cas_root) {
            Ok(ctx) => ctx,
            Err(e) => {
                // Check if fallback is enabled
                let ai_fallback = config.hooks.as_ref().map(|h| h.ai_fallback).unwrap_or(true);
                if ai_fallback {
                    eprintln!("cas: AI context failed ({e}), falling back to standard");
                    build_context(input, 5, cas_root)?
                } else {
                    eprintln!("cas: AI context failed: {e}");
                    return Err(e);
                }
            }
        }
    } else {
        build_context(input, 5, cas_root)?
    };

    // Inject codemap + project-overview freshness warnings.
    //
    // High-severity warnings (missing / significantly stale / any staleness for
    // supervisors) are **prepended** so they land inside the truncated
    // SessionStart preview window the agent skims first. Info-level warnings
    // are appended.
    //
    // Codemap runs first and wins the top slot when both would prepend;
    // project-overview always appends to preserve codemap's ordering dominance
    // when both are high-severity.
    let agent_role = std::env::var("CAS_AGENT_ROLE").ok();
    let is_supervisor = agent_role.as_deref() == Some("supervisor");

    let context = if let Some(staleness) =
        crate::hooks::handlers::handlers_events::check_codemap_freshness(cas_root)
    {
        let codemap_ctx = staleness.format_injection(is_supervisor);
        if context.is_empty() {
            codemap_ctx
        } else if staleness.is_high_severity(is_supervisor) {
            format!("{codemap_ctx}\n{context}")
        } else {
            format!("{context}\n{codemap_ctx}")
        }
    } else {
        context
    };

    let context = if let Some(repo_root) = cas_root.parent() {
        match crate::hooks::handlers::handlers_events::project_overview::check_freshness(
            repo_root,
            agent_role.as_deref(),
        ) {
            Ok(Some(staleness)) => {
                let overview_ctx = staleness.format_injection(is_supervisor);
                if context.is_empty() {
                    overview_ctx
                } else {
                    // Always append so codemap retains the preview top slot
                    // when both modules report high severity.
                    format!("{context}\n{overview_ctx}")
                }
            }
            Ok(None) => context,
            Err(e) => {
                eprintln!("cas: project-overview freshness check failed: {e}");
                context
            }
        }
    } else {
        context
    };

    // Factory session-start hygiene triage (task cas-aeec): for supervisor
    // sessions, append a banner listing uncommitted files in the main
    // worktree with per-file last-touching-task-id attribution. Visibility
    // only — the supervisor decides salvage / commit / discard before
    // spawning workers. Best-effort: git failures, non-supervisor roles,
    // and clean trees all fall through silently.
    //
    // Appended (not prepended) so codemap and project-overview retain the
    // preview top slot they are explicitly engineered to land in (see
    // comments above). The banner is not severity-ranked against those
    // modules, so it sits below them in the supervisor's initial view.
    let context = if is_supervisor {
        match crate::hooks::handlers::session_hygiene::build_session_start_wip_banner(cas_root) {
            Some(banner) if context.is_empty() => banner,
            Some(banner) => format!("{context}\n{banner}"),
            None => context,
        }
    } else {
        context
    };

    // Phase 3 / cas-3efe: opt-in integrations staleness banner. Default
    // off — only fires when `[integrations] session_start_warn = true` in
    // .cas/config.toml *and* at least one platform reports a `Stale` ID.
    // Appended last so it sits below codemap / project-overview / WIP.
    // Reuses the already-loaded `config` from earlier in this handler.
    let context = match build_integrations_session_start_banner(cas_root, &config) {
        Some(banner) if context.is_empty() => banner,
        Some(banner) => format!("{context}\n{banner}"),
        None => context,
    };

    let output = if context.is_empty() {
        HookOutput::empty()
    } else {
        HookOutput::with_session_start_context(context.clone())
    };

    // Record trace if dev mode is enabled
    if let Some(tracer) = DevTracer::get() {
        if tracer.should_trace_hooks() {
            let input_json = serde_json::json!({
                "session_id": input.session_id,
                "cwd": input.cwd,
                "permission_mode": input.permission_mode,
            });
            let output_json = serde_json::json!({
                "has_context": !context.is_empty(),
                "context_length": context.len(),
            });

            let _ = tracer.record_hook(
                "SessionStart",
                &input_json,
                &output_json,
                if context.is_empty() {
                    None
                } else {
                    Some(&context)
                },
                Some(estimate_tokens(&context)),
                timer.elapsed_ms(),
                true,
                None,
            );
        }
    }

    Ok(output)
}

/// Estimate token count (rough approximation: ~4 chars per token)
pub(crate) fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
}

/// Build the opt-in Phase 3 (cas-3efe) integrations banner.
///
/// Returns `None` unless **all three** conditions hold:
/// 1. `config.integrations.session_start_warn == true` (project-level
///    `.cas/config.toml`; the spec scopes the flag to project config).
/// 2. The repo root resolves (cas_root parent).
/// 3. At least one platform's [`crate::cli::integrate::types::VerifyReport`]
///    returns `has_stale() == true`.
///
/// `McpUnreachable` and `not_configured` are deliberately silent here: they
/// aren't actionable enough to displace the codemap freshness banner that
/// shares this slot. Failures during reading/verifying are swallowed —
/// SessionStart should never block on a misconfigured integration.
///
/// Takes the already-loaded [`Config`](crate::config::Config) by reference
/// rather than reloading from disk, so the SessionStart hook only parses
/// `config.toml` once per fire.
pub(crate) fn build_integrations_session_start_banner(
    cas_root: &Path,
    config: &crate::config::Config,
) -> Option<String> {
    let opt_in = config
        .integrations
        .as_ref()
        .map(|i| i.session_start_warn)
        .unwrap_or(false);
    if !opt_in {
        return None;
    }
    let repo_root = cas_root.parent()?;
    let reports = crate::cli::integrate::doctor::collect_reports(repo_root);
    let body = crate::cli::integrate::doctor::session_start_banner_text(&reports, true)?;
    let safe_body = escape_xml_text(&body);
    Some(format!(
        "<integrations-freshness severity=\"info\">\n{safe_body}\n</integrations-freshness>"
    ))
}

/// Minimal XML-text escape so a recorded platform ID containing `<`, `>`,
/// `&`, `"`, or `'` cannot mis-close the wrapper tag (or inject an
/// attribute into the opening tag). Used only for SessionStart banner
/// bodies whose content is platform-supplied via SKILL.md keep blocks.
fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Compute session outcome based on metrics and friction events
///
/// Outcome determination priority:
/// Handle SessionEnd hook - generate session summary and mark for extraction
pub fn handle_session_end(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    let mut stores = HookStores::new(cas_root);

    // Get observations from this session
    let entry_store = stores.entries()?;
    let entries = entry_store.list()?;
    let session_observations: Vec<_> = entries
        .iter()
        .filter(|e| e.session_id.as_deref() == Some(&input.session_id))
        .collect();

    let session_count = session_observations.len();

    // Clean up agent leases and reset task status - ALWAYS do this regardless of observation count
    cleanup_agent_leases(cas_root, &input.session_id);

    // Factory session hygiene (task cas-a9ab): append a durable manifest of
    // the main worktree's uncommitted state so the next supervisor can see
    // what was left behind if this session died mid-task. Best-effort —
    // never let hygiene logging break session-end.
    {
        let agent_name = std::env::var("CAS_AGENT_NAME").ok();
        let agent_role = std::env::var("CAS_AGENT_ROLE").ok();
        if let Some(path) = crate::hooks::handlers::session_hygiene::write_session_end_manifest(
            cas_root,
            &input.session_id,
            agent_name.as_deref(),
            agent_role.as_deref(),
        ) {
            eprintln!(
                "cas: Wrote session-end manifest to {}",
                path.display()
            );
        }
    }

    // Notify daemon via socket that session ended
    #[cfg(feature = "mcp-server")]
    {
        use crate::agent_id::get_cc_pid_for_hook;
        use crate::mcp::socket::{DaemonEvent, send_event};
        let cc_pid = get_cc_pid_for_hook();
        let event = DaemonEvent::SessionEnd {
            session_id: input.session_id.clone(),
            cc_pid: Some(cc_pid),
        };
        if send_event(cas_root, &event).is_ok() {
            eprintln!("cas: Notified daemon of session end");
        }
    }

    // Clean up current_session file
    let _ = std::fs::remove_file(cas_root.join("current_session"));

    // Clean up session files used for context boosting
    clear_session_files(cas_root);

    // Clean up OTEL context file
    let _ = OtelContext::remove(cas_root);

    // Clean up verifier marker file (safety cleanup in case subagent didn't clean up)
    let _ = std::fs::remove_file(cas_root.join(".verifier_unjail_marker"));

    if session_count == 0 {
        eprintln!(
            "cas: Session {} ended (no observations)",
            &input.session_id[..8.min(input.session_id.len())]
        );
        return Ok(HookOutput::empty());
    }

    // Log session end
    eprintln!(
        "cas: Session {} ended with {} observations",
        &input.session_id[..8.min(input.session_id.len())],
        session_count
    );

    // Check if AI features are enabled
    let config = Config::load(cas_root).unwrap_or_default();
    let should_summarize = config
        .hooks
        .as_ref()
        .map(|h| h.generate_summaries)
        .unwrap_or(false);

    // Generate session title and compute outcome (reuses single SqliteStore)
    if let Some(sqlite_store) = stores.sqlite() {
        match generate_session_title_sync(&session_observations) {
            Ok(title) => {
                if sqlite_store
                    .update_session_title(&input.session_id, &title)
                    .is_ok()
                {
                    eprintln!("cas: Session title: {title}");
                }
            }
            Err(e) => {
                eprintln!("cas: Title generation failed: {e}");
            }
        }

        // Compute session outcome
        let session_opt = sqlite_store.get_session(&input.session_id).ok().flatten();

        let outcome = if let Some(session) = session_opt {
            if session.tasks_closed > 0 {
                cas_types::SessionOutcome::TasksCompleted
            } else if session.entries_created > 0 {
                cas_types::SessionOutcome::LearningsCreated
            } else if session.tool_uses > 0 {
                cas_types::SessionOutcome::Exploration
            } else {
                cas_types::SessionOutcome::Abandoned
            }
        } else if session_count > 0 {
            cas_types::SessionOutcome::Exploration
        } else {
            cas_types::SessionOutcome::Abandoned
        };

        if sqlite_store
            .update_session_signals(&input.session_id, Some(outcome), None, None)
            .is_ok()
        {
            eprintln!("cas: Session outcome: {outcome}");
        }
    }

    if should_summarize {
        // Generate summary
        let entry_store = stores.entries()?;
        {
            if let Ok(summary) = generate_session_summary_sync(&session_observations) {
                // Store the summary as a context entry
                if !summary.summary.is_empty() {
                    let id = entry_store.generate_id()?;
                    let mut content = format!("## Session Summary\n\n{}\n", summary.summary);

                    if !summary.decisions.is_empty() {
                        content.push_str("\n### Decisions\n");
                        for decision in &summary.decisions {
                            content.push_str(&format!("- {decision}\n"));
                        }
                    }

                    if !summary.key_learnings.is_empty() {
                        content.push_str("\n### Learnings\n");
                        for learning in &summary.key_learnings {
                            content.push_str(&format!("- {learning}\n"));
                        }
                    }

                    if !summary.follow_up_tasks.is_empty() {
                        content.push_str("\n### Follow-up Tasks\n");
                        for task in &summary.follow_up_tasks {
                            content.push_str(&format!("- {task}\n"));
                        }
                    }

                    let entry = Entry {
                        id: id.clone(),
                        entry_type: EntryType::Context,
                        content,
                        tags: vec!["session-summary".to_string()],
                        session_id: Some(input.session_id.clone()),
                        ..Default::default()
                    };

                    if entry_store.add(&entry).is_ok() {
                        eprintln!("cas: Generated session summary: {id}");
                    }
                }
            }
        }
    }

    Ok(HookOutput::empty())
}

/// Generate session summary using AI (synchronous wrapper with timeout)
pub(crate) fn generate_session_summary_sync(
    observations: &[&Entry],
) -> Result<SessionSummary, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    // 5 second timeout to prevent blocking the hook for too long
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            generate_session_summary_async(observations),
        )
        .await
        .map_err(|_| MemError::Other("AI summary generation timed out after 5s".to_string()))?
    })
}

/// Generate session summary using AI
async fn generate_session_summary_async(
    observations: &[&Entry],
) -> Result<SessionSummary, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    // Build prompt from observations
    let obs_text: String = observations
        .iter()
        .take(50) // Limit to prevent token overflow
        .map(|e| {
            format!(
                "- [{}] {}",
                e.source_tool.as_deref().unwrap_or("?"),
                e.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_text = format!(
        r#"Analyze these observations from a coding session and generate a structured summary.

## Observations
{obs_text}

## Task
Generate a JSON summary with:
- summary: 1-2 sentence overview of what was accomplished
- decisions: Array of key decisions made (architectural, design, etc.)
- tasks_completed: Array of tasks that were finished
- key_learnings: Array of important discoveries or patterns learned
- follow_up_tasks: Array of suggested next tasks

Respond with JSON only, no markdown:
{{"summary": "...", "decisions": [...], "tasks_completed": [...], "key_learnings": [...], "follow_up_tasks": [...]}}"#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "session_summary",
    )
    .await
    .map_err(|e| MemError::Other(format!("AI summary failed: {e}")))?;

    let response_text = result.text();

    // Parse JSON response
    let json_str = response_text
        .find('{')
        .and_then(|start| {
            response_text
                .rfind('}')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or(response_text);

    serde_json::from_str(json_str)
        .map_err(|e| MemError::Parse(format!("Failed to parse summary: {e}")))
}

/// Generate session title (synchronous wrapper with timeout)
pub fn generate_session_title_sync(observations: &[&Entry]) -> Result<String, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    // 15 second timeout - claude CLI spawn can take a few seconds
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(15),
            generate_session_title_async(observations),
        )
        .await
        .map_err(|_| MemError::Other("Title generation timed out after 15s".to_string()))?
    })
}

/// Generate a concise session title using AI
async fn generate_session_title_async(observations: &[&Entry]) -> Result<String, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    if observations.is_empty() {
        return Ok("Empty session".to_string());
    }

    // Build a brief summary of what happened
    let obs_text: String = observations
        .iter()
        .take(20) // Limit to key observations
        .map(|e| {
            let tool = e.source_tool.as_deref().unwrap_or("?");
            let content = truncate_display(&e.content, 100);
            format!("- [{tool}] {content}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_text = format!(
        r#"Generate a 5-8 word title summarizing this coding session.

## Session Activity
{obs_text}

## Examples of good titles:
- "Implemented user authentication flow"
- "Fixed payment processing bug"
- "Refactored database queries for performance"
- "Added dark mode support"
- "Set up CI/CD pipeline"

Respond with ONLY the title, no quotes or punctuation at the end."#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "session_title",
    )
    .await
    .map_err(|e| MemError::Other(format!("Title generation failed: {e}")))?;

    let title = result.text().trim().to_string();

    // Clean up the title - remove quotes if present
    let title = title.trim_matches('"').trim_matches('\'').to_string();

    // Ensure reasonable length
    if title.chars().count() > 100 {
        Ok(title.chars().take(100).collect())
    } else if title.is_empty() {
        Ok("Coding session".to_string())
    } else {
        Ok(title)
    }
}

/// Extract learnings from transcript (synchronous wrapper with timeout)
pub(crate) fn extract_learnings_sync(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<ExtractedLearning>, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    // 5 second timeout to prevent blocking the hook for too long
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            extract_learnings_async(transcript_path, file_paths),
        )
        .await
        .map_err(|_| MemError::Other("Learning extraction timed out after 5s".to_string()))?
    })
}

/// Extract learnings from transcript using AI
///
/// Reads the transcript, sends to Haiku to identify project conventions
/// that the user taught Claude during the session.
async fn extract_learnings_async(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<ExtractedLearning>, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    // Read the transcript file
    let transcript = std::fs::read_to_string(transcript_path)
        .map_err(|e| MemError::Other(format!("Failed to read transcript: {e}")))?;

    // Skip if transcript is too short (likely no meaningful interaction)
    if transcript.len() < 500 {
        return Ok(vec![]);
    }

    // Truncate transcript if too long (keep last 50k chars - most recent context)
    // Find a valid char boundary to avoid slicing in the middle of multi-byte UTF-8 chars
    let transcript_excerpt = if transcript.len() > 50000 {
        let mut start = transcript.len() - 50000;
        // Walk forward to find a valid UTF-8 char boundary
        while start < transcript.len() && !transcript.is_char_boundary(start) {
            start += 1;
        }
        &transcript[start..]
    } else {
        &transcript
    };

    // Build file context from observed paths
    let file_context = if file_paths.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Files Modified This Session\n{}",
            file_paths
                .iter()
                .take(20)
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let prompt_text = format!(
        r#"Analyze this Claude Code session transcript and extract project-specific rules or conventions that the USER TAUGHT Claude.

## What to Look For
- User corrections: "No, don't do X, instead do Y"
- User preferences: "Always use X pattern", "Never import from Y"
- API corrections: "That function doesn't exist, use Z instead"
- Framework conventions: "In this project we use X for Y"
- Style rules: "We don't use useEffect here", "Always use generated types"

## What to IGNORE
- General programming knowledge (not project-specific)
- Claude's own discoveries without user confirmation
- One-off fixes that aren't conventions
- Debugging steps

## Transcript
{transcript_excerpt}
{file_context}

## Task
Extract 0-5 project-specific rules the user taught. For each, include:
- content: The rule in imperative form ("Use X", "Never Y", "Always Z")
- path_pattern: Glob pattern for files this applies to (e.g., "**/*.tsx", "lib/**/*.ex") or null if global
- confidence: 0.7-1.0 based on how explicit the user was
- tags: Relevant tags like ["react", "elixir", "testing"]

Respond with JSON array only, no markdown:
[{{"content": "...", "path_pattern": "...", "confidence": 0.9, "tags": ["..."]}}]

If no clear learnings found, respond with: []"#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "learning_extraction",
    )
    .await
    .map_err(|e| MemError::Other(format!("Learning extraction failed: {e}")))?;

    let response_text = result.text();

    // Parse JSON response
    let json_str = response_text
        .find('[')
        .and_then(|start| {
            response_text
                .rfind(']')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or("[]");

    let learnings: Vec<ExtractedLearning> = serde_json::from_str(json_str)
        .map_err(|e| MemError::Parse(format!("Failed to parse learnings: {e}")))?;

    // Filter out low-confidence learnings
    Ok(learnings
        .into_iter()
        .filter(|l| l.confidence >= 0.7)
        .collect())
}

// ─── session-learn: 7-signal memory classifier (cas-6156 / EPIC cas-ebea) ─────

/// Run the session-learn 7-signal classifier against the transcript.
///
/// Synchronous wrapper — creates a `tokio::Runtime`, calls `session_learn_async`
/// with a 30-second timeout (longer than `extract_learnings_sync` because the
/// 7-signal prompt is richer), and returns the draft list.
///
/// Callers in `stop_flow.rs` apply the confidence gate and overlap-detection
/// (`find_similar_entry`) before writing survivors to the store.
pub(crate) fn session_learn_sync(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<SessionLearnDraft>, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(30),
            session_learn_async(transcript_path, file_paths),
        )
        .await
        .map_err(|_| MemError::Other("session-learn timed out after 30s".to_string()))?
    })
}

/// Async implementation — reads transcript, builds the 7-signal prompt, calls
/// Haiku, and parses the returned JSON array into `Vec<SessionLearnDraft>`.
async fn session_learn_async(
    transcript_path: &str,
    file_paths: &[String],
) -> Result<Vec<SessionLearnDraft>, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    let transcript = std::fs::read_to_string(transcript_path)
        .map_err(|e| MemError::Other(format!("session-learn: cannot read transcript: {e}")))?;

    // Skip trivial transcripts — same guard the SKILL.md documents
    if transcript.len() < 500 {
        return Ok(vec![]);
    }

    // Keep the most-recent 50 k chars (valid UTF-8 boundary)
    let transcript_excerpt = if transcript.len() > 50_000 {
        let mut start = transcript.len() - 50_000;
        while start < transcript.len() && !transcript.is_char_boundary(start) {
            start += 1;
        }
        &transcript[start..]
    } else {
        &transcript
    };

    let file_context = if file_paths.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Files Modified This Session\n{}",
            file_paths
                .iter()
                .take(20)
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let prompt_text = format!(
        r#"You are analyzing a Claude Code session transcript to extract structured memory entries using a 7-signal taxonomy.

## 7-Signal Classification

For each finding, assign exactly one signal:
1. concept  — a new domain term or abstraction the agent learned
2. entity   — a person, project, tool, repo, or library worth remembering for future recall
3. correction — the user pushed back on the agent; this should bind future behavior
4. pattern  — a recurring pitfall, gotcha, or "I always forget X" moment
5. idea     — a proposal that was floated but not acted on (worth saving)
6. decision — an architectural/process/scope decision with a rationale that should outlive the session
7. gap      — something the agent didn't know but should have

## Output Schema

Return a JSON array of draft objects (possibly empty):
[{{
  "signal": "correction",
  "entry_type": "preference",
  "scope": "global",
  "tags": ["correction", "topic"],
  "content": "<imperative-form memory, e.g. 'Always X' or 'Never Y'>",
  "confidence": 0.85,
  "dedup_hits": [],
  "notes": "<optional rationale for non-obvious choices>"
}}]

Default signal → entry_type mapping (override when a better fit is clear):
- concept   → learning    (scope: project if term is codebase-specific, global if cross-project)
- entity    → context     (scope: project)
- correction → preference (scope: global — corrections outlive projects; project only if codebase-specific)
- pattern   → learning    (scope: project if codebase-specific, global if tool-general)
- idea      → context     (scope: project)
- decision  → context     (scope: project)
- gap       → observation (scope: project)

## Quality Rules

- Only emit project-, user-, or session-specific findings — no general programming knowledge
- Emit corrections at confidence >= 0.5; all other signals at confidence >= 0.6
- One signal per draft (a finding that fits two signals = two drafts)
- dedup_hits: list IDs of near-duplicate existing memories if you know them; otherwise []
- Return [] if the session contains no clear signal-worthy findings

## Transcript
{transcript_excerpt}
{file_context}

Return only the JSON array, no prose, no markdown wrapper."#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "session_learn",
    )
    .await
    .map_err(|e| MemError::Other(format!("session-learn LLM call failed: {e}")))?;

    let response_text = result.text();

    // Extract JSON array from the response
    let json_str = response_text
        .find('[')
        .and_then(|start| {
            response_text
                .rfind(']')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or("[]");

    let drafts: Vec<SessionLearnDraft> = serde_json::from_str(json_str)
        .map_err(|e| MemError::Parse(format!("session-learn: failed to parse drafts: {e}")))?;

    Ok(drafts)
}

#[cfg(test)]
mod session_learn_tests {
    use super::*;

    /// Confirm `SessionLearnDraft` round-trips through JSON correctly.
    /// This exercises the serde mapping without a live LLM.
    #[test]
    fn session_learn_draft_deserializes_from_json() {
        let json = r#"[
          {
            "signal": "correction",
            "entry_type": "preference",
            "scope": "global",
            "tags": ["correction", "scope-discipline"],
            "content": "When a worker flags a real gap, amend the AC rather than working around it.",
            "confidence": 0.9,
            "dedup_hits": []
          },
          {
            "signal": "pattern",
            "entry_type": "learning",
            "scope": "project",
            "tags": ["pattern", "git"],
            "content": "Single-commit branches self-cert through the verification gate; multi-commit stacks hit jail.",
            "confidence": 0.85,
            "dedup_hits": [],
            "notes": "Confirmed by cas-8edb"
          }
        ]"#;

        let drafts: Vec<SessionLearnDraft> =
            serde_json::from_str(json).expect("draft JSON must parse");
        assert_eq!(drafts.len(), 2);

        let correction = &drafts[0];
        assert_eq!(correction.signal, "correction");
        assert_eq!(correction.entry_type, "preference");
        assert_eq!(correction.scope, "global");
        assert!((correction.confidence - 0.9).abs() < f32::EPSILON);
        assert!(correction.dedup_hits.is_empty());
        assert!(correction.notes.is_none());

        let pattern = &drafts[1];
        assert_eq!(pattern.signal, "pattern");
        assert_eq!(pattern.notes.as_deref(), Some("Confirmed by cas-8edb"));
    }

    /// Empty-array response is valid and must not error.
    #[test]
    fn session_learn_draft_accepts_empty_array() {
        let drafts: Vec<SessionLearnDraft> =
            serde_json::from_str("[]").expect("empty array must parse");
        assert!(drafts.is_empty());
    }

    /// `session_learn_sync` on a too-short transcript must return Ok([]) without
    /// attempting an LLM call (the < 500 byte guard in session_learn_async).
    /// We verify this by pointing at a real temp file with tiny content.
    #[test]
    fn session_learn_sync_skips_trivial_transcript() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        writeln!(tmp, "short").expect("write");
        let path = tmp.path().to_str().unwrap().to_string();

        let result = session_learn_sync(&path, &[]);
        assert!(
            result.is_ok(),
            "trivial transcript must return Ok, not Err: {result:?}"
        );
        assert!(
            result.unwrap().is_empty(),
            "trivial transcript must return empty draft list"
        );
    }
}
