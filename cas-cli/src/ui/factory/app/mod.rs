//! Factory application state and orchestration

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use cas_mux::{Mux, PaneKind};
use ratatui::layout::Rect;

use super::director::{
    DiffLine, DirectorData, DirectorEvent, DirectorEventDetector, DirectorStores, PanelAreas,
    Prompt, SidecarFocus, ViewMode, generate_prompt,
};
use crate::store::open_prompt_queue_store;
use crate::types::Worktree;
use crate::ui::factory::input::{InputMode, LayoutSizes};
use crate::ui::factory::layout::{FactoryLayout, PaneGrid};
use crate::ui::factory::notification::Notifier;
use crate::ui::theme::ActiveTheme;
use crate::ui::widgets::TreeItemType;
use crate::worktree::WorktreeManager;

mod imports;
mod init;
mod panels_and_modes;
mod render_and_ops;
mod sidecar_and_selection;

// Re-export from cas-factory for backward compatibility
pub use cas_factory::{AutoPromptConfig, EpicState, FactoryConfig};

// Re-export scroll dispatch types so callers in sibling crates can use them
// without reaching into the private `sidecar_and_selection` submodule.
pub use sidecar_and_selection::{
    SCROLL_DOWN_ARROWS, SCROLL_LINES, SCROLL_UP_ARROWS, ScrollAction,
};

/// Booting state for a worker that is being spawned (after prepare, before finish)
#[derive(Debug, Clone)]
pub struct PendingWorkerState {
    /// Worker name
    pub name: String,
    /// When this worker entered the pending state
    pub started_at: Instant,
    /// Whether this spawn is using worktree isolation
    pub isolate: bool,
}

/// Worktree preparation data (can be sent to background thread)
pub struct WorktreePrep {
    pub worktree_path: PathBuf,
    pub branch_name: String,
    pub parent_branch: String,
    pub repo_root: PathBuf,
    pub cas_dir: PathBuf,
}

/// Data needed to spawn a worker (phase 1 output, can be sent to background thread)
pub struct WorkerSpawnPrep {
    pub worker_name: String,
    pub worktree_info: Option<WorktreePrep>,
}

/// Result of background worktree preparation (phase 2 output)
pub struct WorkerSpawnResult {
    pub worker_name: String,
    pub cwd: PathBuf,
    pub cas_root: Option<PathBuf>,
    pub worktree: Option<Worktree>,
}

impl WorkerSpawnPrep {
    /// Phase 2: Run the slow git operations (designed for spawn_blocking).
    pub fn run(self) -> anyhow::Result<WorkerSpawnResult> {
        if let Some(wt) = self.worktree_info {
            use crate::worktree::GitOperations;

            let git = GitOperations::new(wt.repo_root.clone());

            // Check if worktree already exists on disk (reuse from previous session)
            if wt.worktree_path.exists() {
                let _ = git.init_submodules(&wt.worktree_path);
                // Ensure gitignored config is available (may be missing from prior run)
                crate::worktree::symlink_project_config(
                    &wt.repo_root,
                    &wt.worktree_path,
                );
                let worktree = Worktree::new(
                    Worktree::generate_id(),
                    wt.branch_name,
                    wt.parent_branch,
                    wt.worktree_path.clone(),
                );
                return Ok(WorkerSpawnResult {
                    worker_name: self.worker_name,
                    cwd: wt.worktree_path,
                    cas_root: Some(wt.cas_dir),
                    worktree: Some(worktree),
                });
            }

            // Create parent directory
            if let Some(parent) = wt.worktree_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Create git worktree (THE SLOW PART)
            git.create_worktree(&wt.worktree_path, &wt.branch_name, Some(&wt.parent_branch))?;

            // Symlink .mcp.json and .claude/ so workers get MCP access
            crate::worktree::symlink_project_config(&wt.repo_root, &wt.worktree_path);

            let worktree = Worktree::new(
                Worktree::generate_id(),
                wt.branch_name,
                wt.parent_branch,
                wt.worktree_path.clone(),
            );

            Ok(WorkerSpawnResult {
                worker_name: self.worker_name,
                cwd: wt.worktree_path,
                cas_root: Some(wt.cas_dir),
                worktree: Some(worktree),
            })
        } else {
            let cwd = std::env::current_dir()?;
            Ok(WorkerSpawnResult {
                worker_name: self.worker_name,
                cwd,
                cas_root: None,
                worktree: None,
            })
        }
    }
}

/// The main factory application
pub struct FactoryApp {
    /// The terminal multiplexer
    pub mux: Mux,
    /// CAS directory for data loading
    cas_dir: PathBuf,
    /// Cached store handles (avoid re-opening on every 2s refresh)
    director_stores: Option<DirectorStores>,
    /// Director panel data
    director_data: DirectorData,
    /// Current input mode
    pub input_mode: InputMode,
    /// Buffer for inject mode text input
    pub inject_buffer: String,
    /// Target pane for injection
    pub inject_target: Option<String>,
    /// Show help overlay
    pub show_help: bool,
    /// Show file changes dialog
    pub show_changes_dialog: bool,
    /// Selected file for changes dialog (source_path, file_path, source_name, agent_name)
    pub changes_dialog_file: Option<(PathBuf, String, String, Option<String>)>,
    /// Scroll offset for changes dialog diff
    pub changes_dialog_scroll: u16,
    /// Cached diff lines for changes dialog
    pub changes_dialog_diff: Vec<DiffLine>,
    /// Show task detail dialog
    pub show_task_dialog: bool,
    /// Selected task ID for task dialog
    pub task_dialog_id: Option<String>,
    /// Scroll offset for task dialog
    pub task_dialog_scroll: u16,
    /// Max scroll offset for task dialog (computed during render)
    pub task_dialog_max_scroll: u16,
    /// Show reminder detail dialog
    pub show_reminder_dialog: bool,
    /// Selected reminder index for reminder dialog
    pub reminder_dialog_idx: Option<usize>,
    /// Scroll offset for reminder dialog
    pub reminder_dialog_scroll: u16,
    /// Show terminal dialog (interactive shell)
    pub show_terminal_dialog: bool,
    /// Name of the shell pane in the mux
    pub terminal_pane_name: Option<String>,
    /// Show feedback dialog
    pub show_feedback_dialog: bool,
    /// Current feedback category
    pub feedback_category: super::input::FeedbackCategory,
    /// Feedback text buffer
    pub feedback_buffer: String,
    /// Last CAS data refresh time
    last_refresh: Instant,
    /// Refresh interval for CAS data
    refresh_interval: Duration,
    /// Last observed CAS DB file fingerprint used for cheap change detection
    last_db_fingerprint: Option<CasDbFingerprint>,
    /// Last git refresh time
    last_git_refresh: Instant,
    /// Interval for expensive git refresh operations
    git_refresh_interval: Duration,
    /// Theme for rendering
    theme: ActiveTheme,
    /// Worker names (for reference)
    worker_names: Vec<String>,
    /// Supervisor name (for reference)
    supervisor_name: String,
    /// Factory session name (for prompt queue isolation)
    factory_session: Option<String>,
    /// Supervisor CLI mode (claude/codex)
    supervisor_cli: cas_mux::SupervisorCli,
    /// Worker CLI mode (claude/codex)
    worker_cli: cas_mux::SupervisorCli,
    /// Error message to display (cleared on next key or after timeout)
    pub error_message: Option<String>,
    /// Number of workers currently being spawned (for loading indicator)
    pub spawning_count: usize,
    /// SELECT mode: client has disabled mouse capture so native terminal
    /// text selection works. Set via F10 toggle on the client.
    pub select_mode: bool,
    /// Workers currently in the spawning pipeline (after prepare, before finish).
    /// These appear as booting placeholder panes in the layout.
    pub pending_workers: Vec<PendingWorkerState>,
    /// When the error message was set (for auto-dismiss)
    error_set_at: Option<Instant>,
    /// Sidebar collapsed state
    pub sidecar_collapsed: bool,
    /// Worktree manager for worker isolation (None if worktrees disabled)
    worktree_manager: Option<WorktreeManager>,
    /// Index of the currently selected worker tab (0-based, used in tabbed mode)
    pub selected_worker_tab: usize,
    /// Use tabbed worker view instead of side-by-side (config preference)
    tabbed_workers: bool,
    /// Actual tabbed mode active (accounts for auto-switch due to space constraints)
    is_tabbed: bool,
    /// Custom layout percentages (None = use defaults)
    pub layout_sizes: Option<LayoutSizes>,
    /// Spatial grid for pane navigation (rebuilt on layout change)
    pane_grid: PaneGrid,
    /// Currently selected pane in pane select mode
    selected_pane: Option<String>,
    /// Event detector for CAS state changes
    event_detector: DirectorEventDetector,
    /// Notification manager
    notifier: Notifier,
    /// Current epic state
    epic_state: EpicState,
    /// Explicit current epic ID — set when supervisor creates/starts an epic.
    /// Takes priority over passive scanning in detect_epic_state().
    current_epic_id: Option<String>,
    /// Sidecar panel focus
    pub sidecar_focus: SidecarFocus,
    /// Sidecar panel scroll/collapse state
    pub panels: super::director::PanelRegistry,
    /// Panel areas for click detection (updated during render)
    panel_areas: PanelAreas,
    /// Current view mode for the sidecar
    pub view_mode: ViewMode,
    /// Scroll offset for detail views
    detail_scroll: u16,
    /// Agent filter (None = show all)
    pub agent_filter: Option<String>,
    /// Cached diff content for FileDiff view (used by search)
    diff_cache: Vec<DiffLine>,
    /// Scroll offset for diff view (legacy, used by search jump)
    diff_scroll: u16,
    /// Parsed diff metadata for DiffWidget rendering
    diff_metadata: Option<cas_diffs::FileDiffMetadata>,
    /// Syntax highlighter for diff rendering
    syntax_highlighter: cas_diffs::highlight::SyntaxHighlighter,
    /// Scroll/hunk navigation state for DiffWidget
    diff_view_state: cas_diffs::widget::DiffViewState,
    /// Diff display style (unified vs split)
    diff_display_style: cas_diffs::iter::DiffStyle,
    /// Inline diff highlighting mode
    diff_inline_mode: cas_diffs::LineDiffType,
    /// Whether to show line numbers in diff view
    diff_show_line_numbers: bool,
    /// Expanded hunk regions (for expanding collapsed context)
    diff_expanded_hunks: std::collections::HashMap<usize, cas_diffs::iter::HunkExpansionRegion>,
    /// Whether all collapsed regions are expanded
    diff_expand_all: bool,
    /// Whether diff search input mode is active
    diff_search_mode: bool,
    /// Current diff search query
    diff_search_query: String,
    /// Line indices that match the search query
    diff_search_matches: Vec<usize>,
    /// Current match index (for n/N navigation)
    diff_search_current: usize,
    /// Collapsed epic IDs (epics whose subtasks are hidden)
    collapsed_epics: HashSet<String>,
    /// Collapsed directory paths in changes panel
    collapsed_dirs: HashSet<String>,
    /// Tree item types for changes panel (for scroll bounds)
    changes_item_types: Vec<TreeItemType>,
    /// Layout areas for click detection (updated during render)
    worker_tab_bar_area: Option<Rect>,
    worker_content_area: Option<Rect>,
    worker_areas: Vec<Rect>,
    supervisor_area: Option<Rect>,
    sidecar_area: Option<Rect>,
    /// Stored terminal dimensions (for daemon mode where crossterm::terminal::size() doesn't work)
    terminal_cols: u16,
    terminal_rows: u16,
    /// Auto-prompting configuration
    auto_prompt: AutoPromptConfig,
    /// Epic branch name (e.g., "epic/add-user-auth") - workers branch from this
    epic_branch: Option<String>,
    /// Whether recording is enabled for this session
    record_enabled: bool,
    /// Session ID for recordings (only set if record_enabled)
    recording_session_id: Option<String>,
    /// When recording started (for computing event timestamps)
    recording_start: Option<Instant>,
    /// Collected events during this session (for export)
    recorded_events: Vec<(Instant, DirectorEvent)>,
    /// UUID for the team lead's Claude Code session (for Teams config.json)
    lead_session_id: Option<String>,
    /// Project directory (for git operations)
    project_dir: PathBuf,
    /// Session ID to pane name mapping for interaction routing
    session_to_pane: HashMap<String, String>,
    /// Last time Ctrl+C was sent to a pane (debounce rapid repeated presses)
    pub last_interrupt_time: Option<Instant>,
    /// Top-level view mode (Panes vs Mission Control)
    pub factory_view_mode: crate::ui::factory::renderer::FactoryViewMode,
    /// Which panel has focus in Mission Control view
    pub mc_focus: crate::ui::factory::renderer::MissionControlFocus,
    /// Mission Control panel areas for click detection (updated during render)
    mc_workers_area: Rect,
    mc_tasks_area: Rect,
    mc_changes_area: Rect,
    mc_activity_area: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CasDbFingerprint {
    db_mtime: Option<SystemTime>,
    wal_mtime: Option<SystemTime>,
}

impl CasDbFingerprint {
    fn from_cas_dir(cas_dir: &std::path::Path) -> Self {
        let db_path = cas_dir.join("cas.db");
        let wal_path = cas_dir.join("cas.db-wal");

        Self {
            db_mtime: file_mtime(&db_path),
            wal_mtime: file_mtime(&wal_path),
        }
    }
}

fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Convert a title to a branch-safe slug
fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(50)
        .collect()
}

/// Create the epic branch name from a title
pub(crate) fn epic_branch_name(title: &str) -> String {
    format!("epic/{}", slugify(title))
}

impl FactoryApp {
    fn filter_director_agents_to_current_session(&mut self) {
        let mut allowed = std::collections::HashSet::with_capacity(self.worker_names.len() + 1);
        for name in &self.worker_names {
            allowed.insert(name.clone());
        }
        allowed.insert(self.supervisor_name.clone());

        self.director_data
            .agents
            .retain(|agent| allowed.contains(&agent.name));
        self.director_data
            .agent_id_to_name
            .retain(|_, name| allowed.contains(name));

        // Filter tasks to active epic's subtasks only (prevents cross-project task leakage).
        // Also keep tasks assigned to current-session workers that have no epic link yet —
        // there's a read race between task list and dependency list queries where a newly
        // created task may not yet have its parent-child dependency visible, causing its
        // `epic` field to be `None`. Dropping those tasks causes the panel to flash empty.
        if let Some(epic_id) = self.epic_state.epic_id() {
            let epic_id = epic_id.to_string();
            let belongs_to_session = |t: &cas_factory::TaskSummary| -> bool {
                t.epic.as_deref() == Some(&epic_id)
                    || (t.epic.is_none()
                        && t.assignee
                            .as_ref()
                            .is_some_and(|a| allowed.contains(a)))
            };
            self.director_data
                .ready_tasks
                .retain(|t| belongs_to_session(t));
            self.director_data
                .in_progress_tasks
                .retain(|t| belongs_to_session(t));
            self.director_data
                .epic_tasks
                .retain(|t| t.id == epic_id);
        }
    }

    /// Check if we should refresh CAS data
    pub fn should_refresh(&self) -> bool {
        self.last_refresh.elapsed() >= self.refresh_interval
    }

    /// Check if automatic prompting is globally enabled
    pub fn auto_prompt_enabled(&self) -> bool {
        self.auto_prompt.enabled
    }

    /// Get the auto-prompt configuration
    pub fn auto_prompt_config(&self) -> &AutoPromptConfig {
        &self.auto_prompt
    }

    /// Refresh CAS data from stores and detect state changes
    ///
    /// Returns a tuple of (prompts, events) for further processing.
    pub fn refresh_data(&mut self) -> anyhow::Result<(Vec<Prompt>, Vec<DirectorEvent>)> {
        let next_fingerprint = CasDbFingerprint::from_cas_dir(&self.cas_dir);
        let db_changed = match self.last_db_fingerprint {
            Some(prev) => prev != next_fingerprint,
            None => true,
        };
        let git_due = !self.director_data.git_loaded
            || self.last_git_refresh.elapsed() >= self.git_refresh_interval;

        let worktree_root = self.worktree_manager.as_ref().map(|m| m.worktree_root());
        if db_changed {
            let loaded = DirectorData::load_with_stores(
                &self.cas_dir,
                worktree_root.as_deref(),
                git_due,
                self.director_stores.as_ref(),
            )?;
            self.director_data =
                merge_director_data_preserving_git(&self.director_data, loaded, git_due);
            if git_due {
                self.last_git_refresh = Instant::now();
            }
        } else if git_due {
            self.director_data.refresh_git_changes_with_stores(
                &self.cas_dir,
                worktree_root.as_deref(),
                self.director_stores.as_ref(),
            )?;
            self.last_git_refresh = Instant::now();
        } else {
            self.last_refresh = Instant::now();
            return Ok((Vec::new(), Vec::new()));
        }

        self.last_db_fingerprint = Some(next_fingerprint);
        self.last_refresh = Instant::now();

        // Sync session_id → pane_name mappings from agent store
        self.sync_session_mappings();

        // Detect state changes BEFORE filtering so new epics are visible to the
        // event detector. This allows EpicStarted to fire and update epic_state,
        // which the filter depends on for subsequent refresh cycles.
        // Pass the currently-tracked epic id so `EpicStarted` is gated on
        // strict improvement: a stray zero-subtask Open-with-branch epic
        // cannot hijack `epic_state` mid-session (see task cas-4181).
        let events = self
            .event_detector
            .detect_changes(&self.director_data, self.epic_state.epic_id());

        // Update epic_state immediately from detected events so the filter below
        // uses the correct epic_id (otherwise a new epic's tasks get filtered out)
        for event in &events {
            if let DirectorEvent::EpicStarted {
                epic_id,
                epic_title,
            } = event
            {
                self.current_epic_id = Some(epic_id.clone());
                self.epic_state = EpicState::Active {
                    epic_id: epic_id.clone(),
                    epic_title: epic_title.clone(),
                };
            }
        }

        // Now filter to current session (agents + tasks scoped to active epic)
        if db_changed {
            self.filter_director_agents_to_current_session();
        }

        // Generate prompts from events (respecting auto-prompt config)
        let prompts: Vec<Prompt> = events
            .iter()
            .filter_map(|event| {
                generate_prompt(
                    event,
                    &self.director_data,
                    &self.supervisor_name,
                    &self.auto_prompt,
                    self.supervisor_cli,
                    self.worker_cli,
                )
            })
            .collect();

        Ok((prompts, events))
    }

    /// Get the focused pane kind
    pub fn focused_kind(&self) -> Option<&PaneKind> {
        self.mux.focused().map(|p| p.kind())
    }

    /// Check if the supervisor pane is focused
    pub fn focused_is_supervisor(&self) -> bool {
        matches!(self.focused_kind(), Some(PaneKind::Supervisor))
    }

    /// Check if a worker pane is focused
    pub fn focused_is_worker(&self) -> bool {
        matches!(self.focused_kind(), Some(PaneKind::Worker))
    }

    /// Check if the focused pane accepts keyboard input
    ///
    /// Supervisor and worker panes accept keyboard input.
    pub fn focused_accepts_input(&self) -> bool {
        matches!(
            self.focused_kind(),
            Some(PaneKind::Supervisor | PaneKind::Worker | PaneKind::Shell)
        )
    }

    /// Get all worker names for layout (real + pending booting workers)
    pub fn layout_worker_names(&self) -> Vec<String> {
        let mut names = self.worker_names.clone();
        for pw in &self.pending_workers {
            if !names.contains(&pw.name) {
                names.push(pw.name.clone());
            }
        }
        names
    }

    /// Check if a worker name is pending (still booting)
    pub fn is_pending_worker(&self, name: &str) -> bool {
        self.pending_workers.iter().any(|pw| pw.name == name)
    }

    /// Add a worker to the pending set (called after prepare_worker_spawn succeeds)
    pub fn add_pending_worker(&mut self, name: String, isolate: bool) {
        self.pending_workers.push(PendingWorkerState {
            name,
            started_at: Instant::now(),
            isolate,
        });
        // Rebuild pane grid is NOT needed — pending workers are not navigable
        // But we do need to sync layout sizes so the boot pane gets space
        let _ = self.sync_pane_sizes();
    }

    /// Remove a worker from the pending set (called on spawn success or failure)
    pub fn remove_pending_worker(&mut self, name: &str) {
        self.pending_workers.retain(|pw| pw.name != name);
    }

    /// Get worker names
    pub fn worker_names(&self) -> &[String] {
        &self.worker_names
    }

    /// Get the number of active workers
    pub fn worker_count(&self) -> usize {
        self.worker_names.len()
    }

    /// Select a worker tab by index (0-based)
    ///
    /// Returns true if the selection changed.
    pub fn select_worker_tab(&mut self, index: usize) -> bool {
        let total = self.layout_worker_names().len();
        if index < total && index != self.selected_worker_tab {
            self.selected_worker_tab = index;
            true
        } else {
            false
        }
    }

    /// Select a worker tab by 1-based number (for keyboard shortcuts)
    ///
    /// Returns true if the selection changed.
    pub fn select_worker_by_number(&mut self, number: usize) -> bool {
        let total = self.layout_worker_names().len();
        if number > 0 && number <= total {
            self.select_worker_tab(number - 1)
        } else {
            false
        }
    }

    /// Get the currently selected worker name
    pub fn selected_worker(&self) -> Option<&str> {
        self.worker_names
            .get(self.selected_worker_tab)
            .map(|s| s.as_str())
    }

    /// Ensure selected_worker_tab is valid after workers change
    fn clamp_selected_worker_tab(&mut self) {
        let total = self.layout_worker_names().len();
        if total > 0 && self.selected_worker_tab >= total {
            self.selected_worker_tab = total - 1;
        }
    }

    /// Get supervisor name
    pub fn supervisor_name(&self) -> &str {
        &self.supervisor_name
    }

    /// Get factory session name (for prompt queue isolation)
    pub fn factory_session(&self) -> Option<&str> {
        self.factory_session.as_deref()
    }

    /// Set factory session name
    pub fn set_factory_session(&mut self, name: String) {
        self.factory_session = Some(name);
    }

    /// Get the worktree manager (if worktrees are enabled)
    pub fn worktree_manager(&self) -> Option<&WorktreeManager> {
        self.worktree_manager.as_ref()
    }

    /// Get the worktree manager mutably (if worktrees are enabled)
    pub fn worktree_manager_mut(&mut self) -> Option<&mut WorktreeManager> {
        self.worktree_manager.as_mut()
    }

    /// Check if worktree-based isolation is enabled
    pub fn worktrees_enabled(&self) -> bool {
        self.worktree_manager.is_some()
    }

    /// Get the lead session ID (UUID for Teams config.json)
    pub fn lead_session_id(&self) -> Option<&str> {
        self.lead_session_id.as_deref()
    }

    /// Get the director data
    pub fn director_data(&self) -> &DirectorData {
        &self.director_data
    }

    /// Get the CAS directory path
    pub fn cas_dir(&self) -> &std::path::Path {
        &self.cas_dir
    }

    /// Get the theme
    pub fn theme(&self) -> &ActiveTheme {
        &self.theme
    }

    /// Get the notifier
    pub fn notifier(&self) -> &Notifier {
        &self.notifier
    }

    /// Send notifications for detected events
    pub fn notify_events(&self, events: &[DirectorEvent]) {
        for event in events {
            self.notifier.notify_event(event);
        }
    }

    /// Set an error message (auto-dismisses after 5 seconds)
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error_message = Some(msg.into());
        self.error_set_at = Some(Instant::now());
    }

    /// Clear the error message
    pub fn clear_error(&mut self) {
        self.error_message = None;
        self.error_set_at = None;
    }

    /// Check if error should be auto-dismissed (after 5 seconds)
    pub fn check_error_timeout(&mut self) {
        if let Some(set_at) = self.error_set_at {
            if set_at.elapsed() >= Duration::from_secs(5) {
                self.clear_error();
            }
        }
    }

    /// Toggle between Panes and MissionControl factory view modes.
    pub fn toggle_factory_view_mode(&mut self) {
        use crate::ui::factory::renderer::FactoryViewMode;
        self.factory_view_mode = match self.factory_view_mode {
            FactoryViewMode::Panes => FactoryViewMode::MissionControl,
            FactoryViewMode::MissionControl => FactoryViewMode::Panes,
        };
    }

    /// Toggle sidebar collapsed state
    pub fn toggle_sidecar_collapsed(&mut self) {
        self.sidecar_collapsed = !self.sidecar_collapsed;
        // Recalculate PTY dimensions to match new layout
        let _ = self.handle_resize(self.terminal_cols, self.terminal_rows);
    }

    /// Handle resize event
    pub fn handle_resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        // Store terminal dimensions
        self.terminal_cols = cols;
        self.terminal_rows = rows;

        // Include pending workers in layout so boot panes get space
        let all_names = self.layout_worker_names();

        // Calculate actual layout areas and resize panes to match
        let area = Rect::new(0, 0, cols, rows);
        let layout = FactoryLayout::calculate_from_names_with_header_rows(
            area,
            &all_names,
            self.tabbed_workers,
            self.sidecar_collapsed,
            self.layout_sizes,
            0,
        );

        // Resize only REAL worker panes (pending workers have no PTY)
        if layout.is_tabbed {
            // Tabbed mode: all workers share the same viewport size
            if let Some(content_area) = layout.worker_content {
                let inner_height = content_area.height.saturating_sub(2);
                let inner_width = content_area.width.saturating_sub(2);

                for name in &self.worker_names {
                    if let Some(pane) = self.mux.get_mut(name) {
                        let _ = pane.resize(inner_height, inner_width);
                    }
                }
            }
        } else {
            // Side-by-side mode: find each real worker's index in the combined list
            for name in &self.worker_names {
                if let Some(idx) = all_names.iter().position(|n| n == name) {
                    if let Some(worker_area) = layout.worker_areas.get(idx) {
                        let inner_height = worker_area.height.saturating_sub(2);
                        let inner_width = worker_area.width.saturating_sub(2);
                        if let Some(pane) = self.mux.get_mut(name) {
                            let _ = pane.resize(inner_height, inner_width);
                        }
                    }
                }
            }
        }

        // Resize supervisor pane
        if let Some(pane) = self.mux.get_mut(&self.supervisor_name) {
            let inner_height = layout.supervisor_area.height.saturating_sub(2);
            let inner_width = layout.supervisor_area.width.saturating_sub(2);
            let _ = pane.resize(inner_height, inner_width);
        }

        Ok(())
    }

    /// Sync pane sizes with current terminal dimensions
    ///
    /// In daemon mode, crossterm::terminal::size() returns a default (80x24) instead
    /// of failing, so we prefer stored dimensions if they're set to something reasonable.
    pub fn sync_pane_sizes(&mut self) -> anyhow::Result<()> {
        // Use stored dimensions if they're set (indicates daemon mode with client-provided size)
        // Only fall back to crossterm if stored dimensions are at default (120x40)
        let (cols, rows) = if self.terminal_cols > 120 || self.terminal_rows > 40 {
            // We have real dimensions from a client resize event
            (self.terminal_cols, self.terminal_rows)
        } else {
            // Try crossterm, but validate the result
            match crossterm::terminal::size() {
                Ok((c, r)) if c > 80 || r > 24 => (c, r),
                _ => (self.terminal_cols, self.terminal_rows),
            }
        };
        tracing::info!(
            "sync_pane_sizes: using {}x{} (stored: {}x{})",
            cols,
            rows,
            self.terminal_cols,
            self.terminal_rows
        );
        self.handle_resize(cols, rows)
    }
}

fn merge_director_data_preserving_git(
    previous: &DirectorData,
    mut loaded: DirectorData,
    git_due: bool,
) -> DirectorData {
    if !git_due && previous.git_loaded {
        loaded.changes = previous.changes.clone();
        loaded.git_loaded = true;
    }
    loaded
}

pub(crate) fn queue_supervisor_intro_prompt(
    cas_dir: &std::path::Path,
    supervisor_name: &str,
    supervisor_cli: cas_mux::SupervisorCli,
    worker_names: &[String],
    factory_session: Option<&str>,
) {
    let worker_list = if worker_names.is_empty() {
        "(none)".to_string()
    } else {
        worker_names.join(", ")
    };
    let prompt = match supervisor_cli {
        cas_mux::SupervisorCli::Codex => format!(
            "Codex supervisor startup:\n\
- Use skills: cas-supervisor, cas-codex-supervisor-checklist\n\
- No hooks: call MCP tools explicitly (tasks/memory/rules/search)\n\
- Do NOT use /cas-start, /cas-context, or /cas-end\n\
- Canonical current workers for this session: {worker_list}\n\
- First steps: mcp__cs__coordination action=whoami; mcp__cs__task action=list task_type=epic; mcp__cs__task action=ready"
        ),
        cas_mux::SupervisorCli::Claude => return,
    };

    if let Ok(queue) = open_prompt_queue_store(cas_dir) {
        if let Some(session) = factory_session {
            let _ = queue.enqueue_with_session("cas", supervisor_name, &prompt, session);
        } else {
            let _ = queue.enqueue("cas", supervisor_name, &prompt);
        }
    }
}

pub(crate) fn queue_codex_worker_intro_prompt(
    cas_dir: &std::path::Path,
    worker_name: &str,
    worker_cli: cas_mux::SupervisorCli,
) {
    match worker_cli {
        cas_mux::SupervisorCli::Codex => {
            // Codex workers now receive startup workflow as the initial codex prompt arg at spawn time.
            // Avoid queue injection here to prevent duplicate or draft-only startup prompts.
        }
        cas_mux::SupervisorCli::Claude => {
            let prompt = format!(
                "You are a CAS factory worker ({worker_name}).\n\
                 \n\
                 Check your assigned tasks: `mcp__cas__task action=mine`\n\
                 \n\
                 See the cas-worker skill for detailed workflow guidance."
            );
            if let Ok(queue) = open_prompt_queue_store(cas_dir) {
                let _ = queue.enqueue("cas", worker_name, &prompt);
            }
        }
    }
}

/// A change in epic state
#[derive(Debug, Clone)]
pub enum EpicStateChange {
    /// An epic was started
    Started {
        epic_id: String,
        epic_title: String,
        previous_state: EpicState,
    },
    /// An epic was completed
    Completed { epic_id: String, epic_title: String },
}

/// Detect the initial epic state from loaded data.
///
/// If `preferred_epic_id` is set (from session metadata or explicit tracking),
/// look it up directly instead of scanning all epics. Falls back to scanning
/// if the preferred epic is not found or is closed.
pub(crate) fn detect_epic_state(
    data: &DirectorData,
    preferred_epic_id: Option<&str>,
) -> EpicState {
    use cas_types::TaskStatus;

    // If we have an explicit epic ID, try to use it directly (skip scanning)
    if let Some(epic_id) = preferred_epic_id {
        if let Some(epic) = data.epic_tasks.iter().find(|e| e.id == epic_id) {
            if epic.status != TaskStatus::Closed {
                return EpicState::Active {
                    epic_id: epic.id.clone(),
                    epic_title: epic.title.clone(),
                };
            }
        }
    }

    // Find an in-progress epic first (highest priority)
    for epic in &data.epic_tasks {
        if epic.status == TaskStatus::InProgress {
            return EpicState::Active {
                epic_id: epic.id.clone(),
                epic_title: epic.title.clone(),
            };
        }
    }

    // Fall back to open epics that have a branch set (auto-created branch).
    // Prefer epics with active subtasks (in-progress > ready) over stale ones.
    // This prevents stale cross-project epics from shadowing the active epic.
    // The picker is shared with the runtime EpicStarted event detector so the
    // two paths cannot disagree on which Open-with-branch epic is "best" —
    // divergence there caused a mid-session hijack bug (see task cas-4181).
    if let Some(best) = crate::ui::factory::director::pick_best_open_branch_epic(
        &data.epic_tasks,
        &data.in_progress_tasks,
        &data.ready_tasks,
    ) {
        return EpicState::Active {
            epic_id: best.id.clone(),
            epic_title: best.title.clone(),
        };
    }

    // Completing state is transitioned to via handle_epic_events() when EpicCompleted fires
    // Initial state detection only identifies Active epics; Completing is a transient state
    EpicState::Idle
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cas_factory::{FileChangeInfo, GitFileStatus, SourceChangesInfo};

    use super::{DirectorData, merge_director_data_preserving_git};

    fn data_with_changes(git_loaded: bool, changes: Vec<SourceChangesInfo>) -> DirectorData {
        DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes,
            git_loaded,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    #[test]
    fn epic_state_update_before_filter_retains_new_epic_tasks() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorEvent, DirectorEventDetector};

        let old_epic_id = "epic-old";
        let new_epic_id = "epic-new";
        let new_epic_title = "New Feature Epic";

        // Simulate director_data with a new Open-with-branch epic and its subtasks
        let mut data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-1".to_string(),
                title: "Subtask of new epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(new_epic_id.to_string()),
                branch: None,
            }],
            in_progress_tasks: vec![TaskSummary {
                id: "task-2".to_string(),
                title: "In-progress subtask".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("worker-1".to_string()),
                task_type: TaskType::Task,
                epic: Some(new_epic_id.to_string()),
                branch: None,
            }],
            epic_tasks: vec![TaskSummary {
                id: new_epic_id.to_string(),
                title: new_epic_title.to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Epic,
                epic: None,
                branch: Some("epic/new-feature".to_string()),
            }],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        // Start with stale epic_state pointing to old epic
        let mut epic_state = EpicState::Active {
            epic_id: old_epic_id.to_string(),
            epic_title: "Old Epic".to_string(),
        };

        // Event detector sees the new epic and fires EpicStarted
        let mut detector = DirectorEventDetector::new(
            vec!["worker-1".to_string()],
            "supervisor".to_string(),
        );
        // Initialize with empty state so detector sees the new epic as new
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        let events = detector.detect_changes(&data, None);

        // Verify EpicStarted was fired
        let epic_started = events.iter().any(|e| {
            matches!(e, DirectorEvent::EpicStarted { epic_id, .. } if epic_id == new_epic_id)
        });
        assert!(epic_started, "EpicStarted event should fire for new epic");

        // THE FIX: Update epic_state from events BEFORE filtering
        for event in &events {
            if let DirectorEvent::EpicStarted {
                epic_id,
                epic_title,
            } = event
            {
                epic_state = EpicState::Active {
                    epic_id: epic_id.clone(),
                    epic_title: epic_title.clone(),
                };
            }
        }

        // Filter tasks to active epic (simulating filter_director_agents_to_current_session)
        if let Some(eid) = epic_state.epic_id() {
            let eid = eid.to_string();
            data.ready_tasks
                .retain(|t| t.epic.as_deref() == Some(&eid));
            data.in_progress_tasks
                .retain(|t| t.epic.as_deref() == Some(&eid));
            data.epic_tasks.retain(|t| t.id == eid);
        }

        // Tasks should be retained because epic_state now points to new epic
        assert_eq!(data.ready_tasks.len(), 1, "ready_tasks should not be empty after filter");
        assert_eq!(data.in_progress_tasks.len(), 1, "in_progress_tasks should not be empty after filter");
        assert_eq!(data.epic_tasks.len(), 1, "epic_tasks should have the new epic");
    }

    #[test]
    fn preserves_previous_changes_when_git_refresh_not_due() {
        let previous = data_with_changes(
            true,
            vec![SourceChangesInfo {
                source_name: "main".to_string(),
                source_path: std::path::PathBuf::from("."),
                agent_name: None,
                changes: vec![FileChangeInfo {
                    file_path: "src/main.rs".to_string(),
                    lines_added: 3,
                    lines_removed: 1,
                    status: GitFileStatus::Modified,
                    staged: false,
                }],
                total_added: 3,
                total_removed: 1,
            }],
        );
        let loaded_without_git = data_with_changes(false, Vec::new());

        let merged = merge_director_data_preserving_git(&previous, loaded_without_git, false);

        assert!(merged.git_loaded);
        assert_eq!(merged.changes.len(), 1);
        assert_eq!(merged.changes[0].source_name, "main");
    }

    #[test]
    fn keeps_loaded_changes_when_git_refresh_is_due() {
        let previous = data_with_changes(true, Vec::new());
        let loaded_with_git = data_with_changes(
            true,
            vec![SourceChangesInfo {
                source_name: "worker-1".to_string(),
                source_path: std::path::PathBuf::from("."),
                agent_name: Some("worker-1".to_string()),
                changes: vec![FileChangeInfo {
                    file_path: "README.md".to_string(),
                    lines_added: 10,
                    lines_removed: 0,
                    status: GitFileStatus::Added,
                    staged: false,
                }],
                total_added: 10,
                total_removed: 0,
            }],
        );

        let merged = merge_director_data_preserving_git(&previous, loaded_with_git, true);

        assert!(merged.git_loaded);
        assert_eq!(merged.changes.len(), 1);
        assert_eq!(merged.changes[0].source_name, "worker-1");
    }

    #[test]
    fn detect_epic_prefers_epic_with_active_subtasks_over_stale() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        let active_epic_id = "cas-active";
        let stale_epic_id = "cas-zzz-stale"; // Higher ID — would win with old heuristic

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_epic_id.to_string()),
                branch: None,
            }],
            in_progress_tasks: vec![TaskSummary {
                id: "task-ip".to_string(),
                title: "In-progress subtask".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("worker-1".to_string()),
                task_type: TaskType::Task,
                epic: Some(active_epic_id.to_string()),
                branch: None,
            }],
            epic_tasks: vec![
                TaskSummary {
                    id: stale_epic_id.to_string(),
                    title: "Stale Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/stale".to_string()),
                },
                TaskSummary {
                    id: active_epic_id.to_string(),
                    title: "Active Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let state = super::detect_epic_state(&data, None);
        match state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(epic_id, active_epic_id,
                    "Should prefer epic with in-progress subtasks, not stale epic with higher ID");
            }
            other => panic!("Expected Active, got {other:?}"),
        }
    }

    #[test]
    fn detect_epic_falls_back_to_ready_subtasks_when_no_in_progress() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        let active_epic_id = "cas-active";
        let stale_epic_id = "cas-zzz-stale";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_epic_id.to_string()),
                branch: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                TaskSummary {
                    id: stale_epic_id.to_string(),
                    title: "Stale Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/stale".to_string()),
                },
                TaskSummary {
                    id: active_epic_id.to_string(),
                    title: "Active Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let state = super::detect_epic_state(&data, None);
        match state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(epic_id, active_epic_id,
                    "Should prefer epic with ready subtasks over stale epic with no subtasks");
            }
            other => panic!("Expected Active, got {other:?}"),
        }
    }

    #[test]
    fn detect_epic_preferred_id_takes_priority_over_heuristic() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        let preferred_id = "cas-preferred";
        let active_id = "cas-active";

        let data = DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: vec![TaskSummary {
                id: "task-ip".to_string(),
                title: "In-progress subtask".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_id.to_string()),
                branch: None,
            }],
            epic_tasks: vec![
                TaskSummary {
                    id: preferred_id.to_string(),
                    title: "Preferred Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/preferred".to_string()),
                },
                TaskSummary {
                    id: active_id.to_string(),
                    title: "Active Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        // Preferred epic should win even though active_id has in-progress subtasks
        let state = super::detect_epic_state(&data, Some(preferred_id));
        match state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(epic_id, preferred_id,
                    "preferred_epic_id should take priority over subtask heuristic");
            }
            other => panic!("Expected Active, got {other:?}"),
        }
    }

    /// Regression test for cas-4181: factory TUI epic hijack.
    ///
    /// Two Open-with-branch epics exist:
    ///   - `epic-aaa` — lex-earlier, has both in-progress and ready subtasks.
    ///   - `epic-zzz` — lex-later, zero subtasks (the would-be hijacker).
    ///
    /// Before this fix, the runtime `EpicStarted` detector used a
    /// "greatest-lex-ID wins" tiebreak that disagreed with the init path's
    /// subtask-count heuristic. That caused `epic-zzz` to hijack the factory
    /// panel mid-session. After the fix both paths share
    /// `pick_best_open_branch_epic` and the active `epic-aaa` wins init;
    /// the strict-improvement gate then blocks `epic-zzz` from firing
    /// `EpicStarted` at all while `epic-aaa` is already the tracked epic.
    #[test]
    fn runtime_detector_does_not_hijack_active_epic_with_stray_open_branch_epic() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorData, DirectorEvent, DirectorEventDetector};

        let active_id = "epic-aaa";
        let hijacker_id = "epic-zzz";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask of active epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_id.to_string()),
                branch: None,
            }],
            in_progress_tasks: vec![TaskSummary {
                id: "task-ip".to_string(),
                title: "In-progress subtask of active epic".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("worker-1".to_string()),
                task_type: TaskType::Task,
                epic: Some(active_id.to_string()),
                branch: None,
            }],
            epic_tasks: vec![
                TaskSummary {
                    id: active_id.to_string(),
                    title: "Active epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                },
                TaskSummary {
                    id: hijacker_id.to_string(),
                    title: "Stray zero-subtask epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/hijacker".to_string()),
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        // Init path: detect_epic_state must prefer the active (lex-earlier,
        // has subtasks) epic over the stray lex-later one.
        let state = super::detect_epic_state(&data, None);
        match &state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(
                    epic_id, active_id,
                    "detect_epic_state must prefer the epic with active subtasks \
                     regardless of lex-ID order (cas-4181 init path)"
                );
            }
            other => panic!("Expected Active, got {other:?}"),
        }

        // Runtime path: event detector sees both epics as new Open-with-branch.
        // With current_epic_id pointing at the active epic, the strict-improvement
        // gate must suppress any EpicStarted for the stray hijacker.
        let mut detector = DirectorEventDetector::new(
            vec!["worker-1".to_string()],
            "supervisor".to_string(),
        );
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        let events = detector.detect_changes(&data, state.epic_id());

        let hijack_started = events.iter().any(|e| {
            matches!(
                e,
                DirectorEvent::EpicStarted { epic_id, .. } if epic_id == hijacker_id
            )
        });
        assert!(
            !hijack_started,
            "EpicStarted must NOT fire for the zero-subtask hijacker epic \
             while an active epic is already tracked (cas-4181 runtime path)"
        );

        // And the active epic itself should also not re-fire — it's already tracked.
        let active_refired = events.iter().any(|e| {
            matches!(
                e,
                DirectorEvent::EpicStarted { epic_id, .. } if epic_id == active_id
            )
        });
        assert!(
            !active_refired,
            "EpicStarted must not refire for the already-tracked active epic"
        );
    }

    /// If the currently-tracked epic is no longer present in `epic_tasks`
    /// (closed, deleted, or cross-project filter drift) the strict-improvement
    /// gate must treat the slot as vacant so a legitimate new Open-with-branch
    /// epic can take over. Regression guard for the cas-4181 adversarial
    /// "ghost current_epic_id freezes TUI" concern.
    #[test]
    fn runtime_detector_recovers_when_tracked_epic_disappears() {
        use cas_factory::TaskSummary;
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorData, DirectorEvent, DirectorEventDetector};

        let new_id = "epic-new";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask of new epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(new_id.to_string()),
                branch: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![TaskSummary {
                id: new_id.to_string(),
                title: "New epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Epic,
                epic: None,
                branch: Some("epic/new".to_string()),
            }],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let mut detector = DirectorEventDetector::new(
            vec!["worker-1".to_string()],
            "supervisor".to_string(),
        );
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        // current_epic_id points at a ghost epic not in data.epic_tasks.
        let events = detector.detect_changes(&data, Some("epic-ghost"));

        let started_for_new = events.iter().any(|e| {
            matches!(
                e,
                DirectorEvent::EpicStarted { epic_id, .. } if epic_id == new_id
            )
        });
        assert!(
            started_for_new,
            "EpicStarted must fire for the legitimate new epic when the \
             tracked current_epic_id refers to a ghost not in epic_tasks"
        );
    }

    /// Sibling sanity test: when no epic is currently tracked (init-time
    /// detect_changes), the runtime detector must pick the *same* best
    /// Open-with-branch epic as `detect_epic_state` — not the lex-greatest.
    #[test]
    fn runtime_detector_picks_same_epic_as_init_when_no_current_epic() {
        use cas_factory::TaskSummary;
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorData, DirectorEvent, DirectorEventDetector};

        let active_id = "epic-aaa";
        let hijacker_id = "epic-zzz";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_id.to_string()),
                branch: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                TaskSummary {
                    id: active_id.to_string(),
                    title: "Active epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                },
                TaskSummary {
                    id: hijacker_id.to_string(),
                    title: "Stray epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/hijacker".to_string()),
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let mut detector = DirectorEventDetector::new(
            vec!["worker-1".to_string()],
            "supervisor".to_string(),
        );
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        let events = detector.detect_changes(&data, None);

        let started_for: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                DirectorEvent::EpicStarted { epic_id, .. } => Some(epic_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            started_for,
            vec![active_id],
            "with no current epic, EpicStarted must fire for the subtask-winning \
             epic, not the lex-greatest one (cas-4181)"
        );
    }
}
