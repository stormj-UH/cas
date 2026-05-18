//! Hook event handlers
//!
//! Implements handlers for each Claude Code hook event.

use std::path::Path;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::error::MemError;
use crate::otel::OtelContext;
use crate::store::{
    RuleStore, SqliteStore, Store, VerificationStore, WorktreeStore, open_agent_store,
    open_commit_link_store, open_file_change_store, open_loop_store, open_prompt_store,
    open_rule_store, open_spec_store, open_store, open_task_store, open_verification_store,
    open_worktree_store,
};
use crate::tracing::{DevTracer, ToolTrace, TraceTimer};
use crate::types::RuleStatus;
use crate::types::{
    Agent, AgentRole, ChangeType, CommitLink, DependencyType, Entry, EntryType, FileChange,
    ObservationType, Prompt, Rule, Session, Task, TaskStatus, TaskType,
};
use cas_core::SearchIndex;

use crate::hooks::transcript::check_promise_in_transcript;

use crate::hooks::context::{build_context, build_context_ai, build_plan_context};
use crate::hooks::types::{HookInput, HookOutput};
use crate::store::{AgentStore, SpecStore, TaskStore};
use std::sync::Arc;

/// Shared store context for hook handlers.
///
/// Opens each store lazily on first use and caches it, avoiding redundant
/// `open_*()` calls (each of which runs `.init()` migrations and `Config::load()`).
pub(crate) struct HookStores<'a> {
    cas_root: &'a Path,
    sqlite: Option<SqliteStore>,
    entry_store: Option<Arc<dyn Store>>,
    task_store: Option<Arc<dyn TaskStore>>,
    agent_store: Option<Arc<dyn AgentStore>>,
}

impl<'a> HookStores<'a> {
    pub fn new(cas_root: &'a Path) -> Self {
        Self {
            cas_root,
            sqlite: None,
            entry_store: None,
            task_store: None,
            agent_store: None,
        }
    }

    /// Get the raw SqliteStore (for session tracking, titles, outcomes)
    pub fn sqlite(&mut self) -> Option<&SqliteStore> {
        if self.sqlite.is_none() {
            if let Ok(store) = SqliteStore::open(self.cas_root) {
                let _ = store.init();
                self.sqlite = Some(store);
            }
        }
        self.sqlite.as_ref()
    }

    /// Get the entry store (for listing entries)
    pub fn entries(&mut self) -> Result<&Arc<dyn Store>, MemError> {
        if self.entry_store.is_none() {
            self.entry_store = Some(open_store(self.cas_root)?);
        }
        Ok(self.entry_store.as_ref().unwrap())
    }

    /// Get the task store
    pub fn tasks(&mut self) -> Option<&Arc<dyn TaskStore>> {
        if self.task_store.is_none() {
            if let Ok(store) = open_task_store(self.cas_root) {
                self.task_store = Some(store);
            }
        }
        self.task_store.as_ref()
    }

    /// Get the agent store
    pub fn agents(&mut self) -> Option<&Arc<dyn AgentStore>> {
        if self.agent_store.is_none() {
            if let Ok(store) = open_agent_store(self.cas_root) {
                self.agent_store = Some(store);
            }
        }
        self.agent_store.as_ref()
    }
}

/// Shared store context for PreToolUse/PostToolUse hook handlers.
///
/// Opens each store lazily on first use and caches it, avoiding redundant
/// `open_*()` calls per tool invocation (each of which runs `.init()` migrations
/// and `Config::load()`). Without caching, a single PreToolUse fires up to ~11
/// separate SQLite connections.
pub(crate) struct ToolHookStores<'a> {
    cas_root: &'a Path,
    config: Option<Config>,
    task_store: Option<Arc<dyn TaskStore>>,
    agent_store: Option<Arc<dyn AgentStore>>,
    verification_store: Option<Arc<dyn VerificationStore>>,
    worktree_store: Option<Arc<dyn WorktreeStore>>,
    rule_store: Option<Arc<dyn RuleStore>>,
    spec_store: Option<Arc<dyn SpecStore>>,
}

impl<'a> ToolHookStores<'a> {
    pub fn new(cas_root: &'a Path) -> Self {
        Self {
            cas_root,
            config: None,
            task_store: None,
            agent_store: None,
            verification_store: None,
            worktree_store: None,
            rule_store: None,
            spec_store: None,
        }
    }

    /// Get or load Config (cached)
    pub fn config(&mut self) -> &Config {
        if self.config.is_none() {
            self.config = Some(Config::load(self.cas_root).unwrap_or_default());
        }
        self.config.as_ref().unwrap()
    }

    /// Get the task store (lazy)
    pub fn tasks(&mut self) -> Option<&Arc<dyn TaskStore>> {
        if self.task_store.is_none() {
            self.task_store = open_task_store(self.cas_root).ok();
        }
        self.task_store.as_ref()
    }

    /// Get the agent store (lazy)
    pub fn agents(&mut self) -> Option<&Arc<dyn AgentStore>> {
        if self.agent_store.is_none() {
            self.agent_store = open_agent_store(self.cas_root).ok();
        }
        self.agent_store.as_ref()
    }

    /// Get the verification store (lazy)
    pub fn verification(&mut self) -> Option<&Arc<dyn VerificationStore>> {
        if self.verification_store.is_none() {
            self.verification_store = open_verification_store(self.cas_root).ok();
        }
        self.verification_store.as_ref()
    }

    /// Get the worktree store (lazy)
    pub fn worktrees(&mut self) -> Option<&Arc<dyn WorktreeStore>> {
        if self.worktree_store.is_none() {
            self.worktree_store = open_worktree_store(self.cas_root).ok();
        }
        self.worktree_store.as_ref()
    }

    /// Get the rule store (lazy)
    pub fn rules(&mut self) -> Result<&Arc<dyn RuleStore>, MemError> {
        if self.rule_store.is_none() {
            self.rule_store = Some(open_rule_store(self.cas_root)?);
        }
        Ok(self.rule_store.as_ref().unwrap())
    }

    /// Get the spec store (lazy)
    pub fn specs(&mut self) -> Option<&Arc<dyn SpecStore>> {
        if self.spec_store.is_none() {
            self.spec_store = open_spec_store(self.cas_root).ok();
        }
        self.spec_store.as_ref()
    }
}

/// Session summary result from AI analysis
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    /// Brief summary of what was accomplished
    pub summary: String,
    /// Key decisions made during the session
    pub decisions: Vec<String>,
    /// Tasks that were completed
    pub tasks_completed: Vec<String>,
    /// Important learnings or discoveries
    pub key_learnings: Vec<String>,
    /// Suggested follow-up tasks
    pub follow_up_tasks: Vec<String>,
}

/// Extracted learning from transcript analysis
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractedLearning {
    /// The rule/convention content
    pub content: String,
    /// File path pattern this applies to (e.g., "**/*.tsx", "lib/cas_cloud_web/**")
    pub path_pattern: Option<String>,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Tags for categorization
    pub tags: Vec<String>,
}

/// Extracted preference from user prompt analysis
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractedPreference {
    /// The rule content in imperative form
    pub content: String,
    /// Scope: "global" (user preference) or "project" (project-specific)
    pub scope: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Optional file path pattern this applies to
    #[serde(default)]
    pub path_pattern: Option<String>,
}

/// Tools worth capturing observations from
const CAPTURE_TOOLS: &[&str] = &["Write", "Edit", "Bash", "Read"];

/// Maximum number of recent files to track per session
const MAX_RECENT_FILES: usize = 10;

pub(crate) fn truncate_display(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

mod handlers_session;
mod handlers_state;
pub(crate) mod session_hygiene;

#[cfg(test)]
pub(crate) use handlers_session::estimate_tokens;
pub(crate) use handlers_session::{extract_learnings_sync, generate_session_summary_sync};
pub use handlers_session::{generate_session_title_sync, handle_session_end, handle_session_start};
pub(crate) use handlers_state::{
    cleanup_agent_leases, cleanup_orphaned_tasks, clear_session_files, current_agent_id,
    detect_significant_activity, extract_activity_entity_id, get_exit_blockers, track_session_file,
};
pub use handlers_state::{get_session_files, handle_subagent_start, handle_subagent_stop};

mod handlers_middle;
pub use handlers_middle::{handle_post_tool_use, handle_stop, handle_user_prompt_submit};
#[cfg(test)]
pub(crate) use handlers_middle::is_file_within_project;

pub(crate) mod handlers_events;
pub use handlers_events::{
    handle_notification, handle_permission_request, handle_pre_compact, handle_pre_tool_use,
};

#[cfg(test)]
mod handlers_tests;

#[cfg(test)]
use handlers_events::*;
#[cfg(test)]
use handlers_middle::*;
