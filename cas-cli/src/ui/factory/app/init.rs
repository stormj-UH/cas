use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cas_mux::{Mux, MuxConfig};
use ratatui::layout::Rect;

use crate::config::Config;
use crate::orchestration::names::generate_unique;
use crate::store::find_cas_root;
use crate::ui::factory::app::{
    AutoPromptConfig, EpicState, FactoryApp, FactoryConfig, detect_epic_state, epic_branch_name,
    queue_codex_worker_intro_prompt, queue_supervisor_intro_prompt,
};
use crate::ui::factory::director::DirectorStores;
use crate::ui::factory::director::{
    DirectorData, DirectorEventDetector, PanelAreas, SidecarFocus, ViewMode,
};
use crate::ui::factory::input::{FeedbackCategory, InputMode};
use crate::ui::factory::layout::PaneGrid;
use crate::ui::factory::notification::Notifier;
use crate::ui::theme::ActiveTheme;
use crate::worktree::{WorktreeConfig, WorktreeManager};

impl FactoryApp {
    /// Create a new factory app with the given configuration.
    pub fn new(config: FactoryConfig) -> anyhow::Result<Self> {
        use crate::worktree::GitOperations;

        let cas_dir = find_cas_root()?;

        let (supervisor_name, worker_names) = if config.minions_theme
            && config.supervisor_name.is_none()
            && config.worker_names.is_empty()
        {
            use crate::orchestration::names::{generate_minion_supervisor, generate_minion_unique};
            let sup = generate_minion_supervisor();
            let workers = generate_minion_unique(config.workers);
            (sup, workers)
        } else {
            let all_names = generate_unique(config.workers + 1);
            let sup = config
                .supervisor_name
                .unwrap_or_else(|| all_names[0].clone());
            let workers = if config.worker_names.is_empty() {
                all_names[1..].to_vec()
            } else {
                config.worker_names
            };
            (sup, workers)
        };

        let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 40));

        let director_data = DirectorData::load_fast(&cas_dir)?;
        let epic_state = detect_epic_state(&director_data, None);

        let epic_branch = if let EpicState::Active { epic_title, .. } = &epic_state {
            let branch_name = epic_branch_name(epic_title);
            let git_ops = GitOperations::new(config.cwd.clone());
            if git_ops.create_branch_if_not_exists(&branch_name)? {
                tracing::info!("Created epic branch: {}", branch_name);
            } else {
                tracing::info!("Using existing epic branch: {}", branch_name);
            }
            Some(branch_name)
        } else {
            None
        };

        let (worktree_manager, worker_cwds, cas_root_for_mux) = if config.enable_worktrees {
            let worktree_root = config
                .worktree_root
                .clone()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| {
                    let cas_config = Config::load(&cas_dir).unwrap_or_default();
                    cas_config
                        .worktrees()
                        .resolve_base_path(&config.cwd)
                        .to_string_lossy()
                        .to_string()
                });

            let wt_config = WorktreeConfig {
                enabled: true,
                base_path: worktree_root,
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            };

            let mut manager = WorktreeManager::new(&config.cwd, wt_config)?;
            let mut cwds = HashMap::new();
            for name in &worker_names {
                let worktree = manager.ensure_worker_worktree(name)?;
                cwds.insert(name.clone(), worktree.path.clone());
            }

            (Some(manager), cwds, Some(cas_dir.clone()))
        } else {
            (None, HashMap::new(), None)
        };

        // Resolve theme: explicit config overrides auto-detection
        let cas_config = Config::load(&cas_dir).unwrap_or_default();
        let theme_config = cas_config.theme;
        let theme = ActiveTheme::resolve(theme_config.as_ref());

        // Register agent colors so TUI renders match Claude Code's Teams colors
        for (name, tc) in &config.teams_configs {
            crate::ui::theme::register_agent_color(name, &tc.agent_color);
        }

        let mux_config = MuxConfig {
            cwd: config.cwd.clone(),
            cas_root: cas_root_for_mux,
            worker_cwds,
            workers: worker_names.len(),
            worker_names: worker_names.clone(),
            supervisor_name: supervisor_name.clone(),
            supervisor_cli: config.supervisor_cli,
            worker_cli: config.worker_cli,
            supervisor_model: config.supervisor_model.clone(),
            worker_model: config.worker_model.clone(),
            supervisor_effort: config.supervisor_effort.clone(),
            worker_effort: config.worker_effort.clone(),
            include_director: false,
            rows,
            cols,
            teams_configs: config.teams_configs,
            resolved_worker_specs: config.resolved_worker_specs,
        };

        // Cache store handles for efficient periodic refresh
        let director_stores = DirectorStores::open(&cas_dir).ok();

        let mut mux = Mux::factory(mux_config)?;
        mux.focus(&supervisor_name);

        let mut event_detector =
            DirectorEventDetector::new(worker_names.clone(), supervisor_name.clone());
        event_detector.initialize(&director_data);

        let notifier = Notifier::new(config.notify);
        let pane_grid = PaneGrid::new(&worker_names, &supervisor_name, config.tabbed_workers);

        let supervisor_name_for_prompt = supervisor_name.clone();
        let app = Self {
            mux,
            cas_dir,
            director_stores,
            director_data,
            input_mode: InputMode::Normal,
            inject_buffer: String::new(),
            inject_target: None,
            show_help: false,
            show_changes_dialog: false,
            changes_dialog_file: None,
            changes_dialog_scroll: 0,
            changes_dialog_diff: Vec::new(),
            show_task_dialog: false,
            task_dialog_id: None,
            task_dialog_scroll: 0,
            task_dialog_max_scroll: 0,
            show_reminder_dialog: false,
            reminder_dialog_idx: None,
            reminder_dialog_scroll: 0,
            show_terminal_dialog: false,
            terminal_pane_name: None,
            show_feedback_dialog: false,
            feedback_category: FeedbackCategory::default(),
            feedback_buffer: String::new(),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            last_db_fingerprint: None,
            last_git_refresh: Instant::now(),
            git_refresh_interval: Duration::from_secs(10),
            theme,
            worker_names,
            supervisor_name,
            factory_session: None,
            supervisor_cli: config.supervisor_cli,
            worker_cli: config.worker_cli,
            error_message: None,
            spawning_count: 0,
            select_mode: false,
            pending_workers: Vec::new(),
            error_set_at: None,
            sidecar_collapsed: false,
            worktree_manager,
            selected_worker_tab: 0,
            tabbed_workers: config.tabbed_workers,
            is_tabbed: config.tabbed_workers,
            layout_sizes: None,
            pane_grid,
            selected_pane: None,
            event_detector,
            notifier,
            current_epic_id: epic_state.epic_id().map(|s| s.to_string()),
            epic_state,
            sidecar_focus: SidecarFocus::None,
            panels: Default::default(),
            panel_areas: PanelAreas::default(),
            view_mode: ViewMode::default(),
            detail_scroll: 0,
            agent_filter: None,
            diff_cache: Vec::new(),
            diff_scroll: 0,
            diff_metadata: None,
            syntax_highlighter: cas_diffs::highlight::SyntaxHighlighter::new(),
            diff_view_state: cas_diffs::widget::DiffViewState::default(),
            diff_display_style: cas_diffs::iter::DiffStyle::Unified,
            diff_inline_mode: cas_diffs::LineDiffType::WordAlt,
            diff_show_line_numbers: true,
            diff_expanded_hunks: std::collections::HashMap::new(),
            diff_expand_all: false,
            diff_search_mode: false,
            diff_search_query: String::new(),
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            collapsed_epics: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            changes_item_types: Vec::new(),
            worker_tab_bar_area: None,
            worker_content_area: None,
            worker_areas: vec![],
            supervisor_area: None,
            sidecar_area: None,
            terminal_cols: cols,
            terminal_rows: rows,
            auto_prompt: config.auto_prompt.clone(),
            epic_branch,
            record_enabled: config.record,
            recording_session_id: config.session_id,
            recording_start: None,
            recorded_events: Vec::new(),
            lead_session_id: config.lead_session_id,
            project_dir: config.cwd.clone(),
            session_to_pane: HashMap::new(),
            last_interrupt_time: None,
            factory_view_mode: crate::ui::factory::renderer::FactoryViewMode::default(),
            mc_focus: crate::ui::factory::renderer::MissionControlFocus::default(),
            mc_workers_area: Rect::default(),
            mc_tasks_area: Rect::default(),
            mc_changes_area: Rect::default(),
            mc_activity_area: Rect::default(),
        };

        queue_supervisor_intro_prompt(
            app.cas_dir(),
            &supervisor_name_for_prompt,
            config.supervisor_cli,
            &app.worker_names,
            None,
        );
        for worker in &app.worker_names {
            queue_codex_worker_intro_prompt(app.cas_dir(), worker, config.worker_cli);
        }

        Ok(app)
    }

    /// Create a FactoryApp from daemon initialization result.
    #[allow(clippy::too_many_arguments)]
    pub fn from_init_result(
        cas_dir: PathBuf,
        mux: Mux,
        worktree_manager: Option<WorktreeManager>,
        director_data: DirectorData,
        supervisor_name: String,
        worker_names: Vec<String>,
        notify_config: crate::ui::factory::notification::NotifyConfig,
        tabbed_workers: bool,
        auto_prompt: AutoPromptConfig,
        supervisor_cli: cas_mux::SupervisorCli,
        worker_cli: cas_mux::SupervisorCli,
        cols: u16,
        rows: u16,
        record_enabled: bool,
        recording_session_id: Option<String>,
        lead_session_id: Option<String>,
        project_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        let mut event_detector =
            DirectorEventDetector::new(worker_names.clone(), supervisor_name.clone());
        event_detector.initialize(&director_data);

        let epic_state = detect_epic_state(&director_data, None);
        let epic_branch = match &epic_state {
            EpicState::Active { epic_title, .. } => Some(epic_branch_name(epic_title)),
            _ => None,
        };

        let notifier = Notifier::new(notify_config);

        // Cache store handles for efficient periodic refresh
        let director_stores = DirectorStores::open(&cas_dir).ok();

        // Resolve theme: explicit config overrides auto-detection
        let cas_config = Config::load(&cas_dir).unwrap_or_default();
        let theme = ActiveTheme::resolve(cas_config.theme.as_ref());

        let app = Self {
            mux,
            cas_dir,
            director_stores,
            director_data,
            input_mode: InputMode::Normal,
            inject_buffer: String::new(),
            inject_target: None,
            show_help: false,
            show_changes_dialog: false,
            changes_dialog_file: None,
            changes_dialog_scroll: 0,
            changes_dialog_diff: Vec::new(),
            show_task_dialog: false,
            task_dialog_id: None,
            task_dialog_scroll: 0,
            task_dialog_max_scroll: 0,
            show_reminder_dialog: false,
            reminder_dialog_idx: None,
            reminder_dialog_scroll: 0,
            show_terminal_dialog: false,
            terminal_pane_name: None,
            show_feedback_dialog: false,
            feedback_category: FeedbackCategory::default(),
            feedback_buffer: String::new(),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            last_db_fingerprint: None,
            last_git_refresh: Instant::now(),
            git_refresh_interval: Duration::from_secs(10),
            theme,
            worker_names: worker_names.clone(),
            supervisor_name: supervisor_name.clone(),
            factory_session: None,
            supervisor_cli,
            worker_cli,
            error_message: None,
            spawning_count: 0,
            select_mode: false,
            pending_workers: Vec::new(),
            error_set_at: None,
            sidecar_collapsed: false,
            worktree_manager,
            selected_worker_tab: 0,
            tabbed_workers,
            is_tabbed: tabbed_workers,
            layout_sizes: None,
            pane_grid: PaneGrid::new(&worker_names, &supervisor_name, tabbed_workers),
            selected_pane: None,
            event_detector,
            notifier,
            current_epic_id: epic_state.epic_id().map(|s| s.to_string()),
            epic_state,
            sidecar_focus: SidecarFocus::None,
            panels: Default::default(),
            panel_areas: PanelAreas::default(),
            view_mode: ViewMode::default(),
            detail_scroll: 0,
            agent_filter: None,
            diff_cache: Vec::new(),
            diff_scroll: 0,
            diff_metadata: None,
            syntax_highlighter: cas_diffs::highlight::SyntaxHighlighter::new(),
            diff_view_state: cas_diffs::widget::DiffViewState::default(),
            diff_display_style: cas_diffs::iter::DiffStyle::Unified,
            diff_inline_mode: cas_diffs::LineDiffType::WordAlt,
            diff_show_line_numbers: true,
            diff_expanded_hunks: std::collections::HashMap::new(),
            diff_expand_all: false,
            diff_search_mode: false,
            diff_search_query: String::new(),
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            collapsed_epics: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            changes_item_types: Vec::new(),
            worker_tab_bar_area: None,
            worker_content_area: None,
            worker_areas: vec![],
            supervisor_area: None,
            sidecar_area: None,
            terminal_cols: cols,
            terminal_rows: rows,
            auto_prompt,
            epic_branch,
            record_enabled,
            recording_session_id,
            recording_start: None,
            recorded_events: Vec::new(),
            lead_session_id,
            project_dir,
            session_to_pane: HashMap::new(),
            last_interrupt_time: None,
            factory_view_mode: crate::ui::factory::renderer::FactoryViewMode::default(),
            mc_focus: crate::ui::factory::renderer::MissionControlFocus::default(),
            mc_workers_area: Rect::default(),
            mc_tasks_area: Rect::default(),
            mc_changes_area: Rect::default(),
            mc_activity_area: Rect::default(),
        };

        queue_supervisor_intro_prompt(
            app.cas_dir(),
            &supervisor_name,
            supervisor_cli,
            &worker_names,
            None,
        );
        for worker in &worker_names {
            queue_codex_worker_intro_prompt(app.cas_dir(), worker, worker_cli);
        }

        Ok(app)
    }
}
