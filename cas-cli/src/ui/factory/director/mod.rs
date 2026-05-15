//! Sidecar panels - displays CAS tasks, agents, changes, and activity
//!
//! This module provides native ratatui widgets for the factory TUI sidecar.
//! The panels are rendered directly without a containing block wrapper,
//! each section has its own header via the compact widget functions.

pub(crate) mod activity;
pub mod agent_helpers;
pub(crate) mod changes;
mod data;
mod events;
mod factory_radar;
pub mod mission_epic;
pub mod mission_workers;
pub mod panel;
mod prompts;
mod reminders;
pub(crate) mod tasks;

pub use data::{AgentSummary, DirectorData, DirectorStores, TaskSummary};
pub use events::{DirectorEvent, DirectorEventDetector};
pub(crate) use events::pick_best_open_branch_epic;
pub use panel::PanelRegistry;
pub use prompts::{Prompt, generate_prompt, with_response_instructions};
// PanelAreas, SidecarFocus, SidecarState, ViewMode, DiffLine, DiffLineType, render, render_with_state are already public in this module

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::ListState;

use crate::ui::theme::ActiveTheme;
use crate::ui::widgets::TreeItemType;

/// Which sidecar panel has focus
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SidecarFocus {
    #[default]
    None,
    Factory,
    Tasks,
    Reminders,
    Changes,
    Activity,
}

/// View mode for the sidecar panel
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ViewMode {
    /// Overview showing all panels
    #[default]
    Overview,
    /// Full task detail view
    TaskDetail(String),
    /// Full activity log view
    ActivityLog,
    /// File diff view (source_path, file_path)
    FileDiff(std::path::PathBuf, String),
}

/// Type of diff line for coloring
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffLineType {
    Context,
    Added,
    Removed,
    HunkHeader,
    FileHeader,
}

/// A processed diff line with line numbers
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub content: String,
    pub line_type: DiffLineType,
}

impl SidecarFocus {
    /// Cycle to the next panel
    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Factory,
            Self::Factory => Self::Tasks,
            Self::Tasks => Self::Reminders,
            Self::Reminders => Self::Changes,
            Self::Changes => Self::Activity,
            Self::Activity => Self::None,
        }
    }

    /// Cycle to the previous panel
    pub fn prev(self) -> Self {
        match self {
            Self::None => Self::Activity,
            Self::Factory => Self::None,
            Self::Tasks => Self::Factory,
            Self::Reminders => Self::Tasks,
            Self::Changes => Self::Reminders,
            Self::Activity => Self::Changes,
        }
    }

    /// Cycle to the next panel, skipping Reminders if there are none
    pub fn next_with_reminders(self, has_reminders: bool) -> Self {
        let next = self.next();
        if next == Self::Reminders && !has_reminders {
            next.next()
        } else {
            next
        }
    }

    /// Cycle to the previous panel, skipping Reminders if there are none
    pub fn prev_with_reminders(self, has_reminders: bool) -> Self {
        let prev = self.prev();
        if prev == Self::Reminders && !has_reminders {
            prev.prev()
        } else {
            prev
        }
    }
}

/// Mutable state for sidecar rendering
pub struct SidecarState<'a> {
    pub focus: SidecarFocus,
    pub tasks_state: &'a mut ListState,
    pub agents_state: &'a mut ListState,
    pub reminders_state: &'a mut ListState,
    pub changes_state: &'a mut ListState,
    pub activity_state: &'a mut ListState,
    /// Optional agent filter (filter tasks/activity by this agent name)
    pub agent_filter: Option<&'a str>,
    /// Section collapse flags
    pub factory_collapsed: bool,
    pub tasks_collapsed: bool,
    pub reminders_collapsed: bool,
    pub changes_collapsed: bool,
    pub activity_collapsed: bool,
    /// Collapsed epic IDs
    pub collapsed_epics: &'a std::collections::HashSet<String>,
    /// Collapsed directory paths in changes panel
    pub collapsed_dirs: &'a std::collections::HashSet<String>,
    /// Output: tree item types from changes panel (for scroll bounds)
    pub changes_item_types: &'a mut Vec<TreeItemType>,
}

/// Panel areas for click detection
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct PanelAreas {
    pub factory: Rect,
    pub tasks: Rect,
    pub reminders: Rect,
    pub changes: Rect,
    pub activity: Rect,
}

/// Render the sidecar panels with optional navigation state
///
/// Returns the panel areas for click detection.
pub fn render_with_state(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    supervisor_name: &str,
    mut state: Option<&mut SidecarState>,
) -> PanelAreas {
    let factory_collapsed = state.as_ref().map(|s| s.factory_collapsed).unwrap_or(false);
    // Get collapse flags
    let tasks_collapsed = state.as_ref().map(|s| s.tasks_collapsed).unwrap_or(false);
    let reminders_collapsed = state
        .as_ref()
        .map(|s| s.reminders_collapsed)
        .unwrap_or(false);
    let changes_collapsed = state.as_ref().map(|s| s.changes_collapsed).unwrap_or(false);
    let activity_collapsed = state
        .as_ref()
        .map(|s| s.activity_collapsed)
        .unwrap_or(false);

    let has_reminders = !data.reminders.is_empty();

    let focus = state
        .as_ref()
        .map(|s| s.focus)
        .unwrap_or(SidecarFocus::None);
    tracing::debug!("render_with_state: focus={:?}, area={:?}", focus, area);

    // Calculate constraints based on collapse state (collapsed = 1 line header only)
    // Reminders panel is only included when there are active reminders
    let mut constraints: Vec<Constraint> = vec![
        if factory_collapsed {
            Constraint::Length(1)
        } else {
            Constraint::Percentage(if has_reminders { 25 } else { 28 })
        },
        if tasks_collapsed {
            Constraint::Length(1)
        } else {
            Constraint::Percentage(if has_reminders { 23 } else { 26 })
        },
    ];

    if has_reminders {
        constraints.push(if reminders_collapsed {
            Constraint::Length(1)
        } else {
            Constraint::Percentage(14)
        });
    }

    constraints.push(if changes_collapsed {
        Constraint::Length(1)
    } else {
        Constraint::Percentage(if has_reminders { 19 } else { 23 })
    });
    constraints.push(if activity_collapsed {
        Constraint::Length(1)
    } else {
        Constraint::Percentage(if has_reminders { 19 } else { 23 })
    });

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let agent_filter = state.as_ref().and_then(|s| s.agent_filter);

    // Get collapsed_epics from state (or empty set if no state)
    #[allow(clippy::incompatible_msrv)]
    static EMPTY_SET: std::sync::LazyLock<std::collections::HashSet<String>> =
        std::sync::LazyLock::new(std::collections::HashSet::new);
    let collapsed_epics = state
        .as_ref()
        .map(|s| s.collapsed_epics)
        .unwrap_or(&EMPTY_SET);

    // Track chunk indices (reminders panel shifts subsequent indices)
    let factory_idx = 0;
    let tasks_idx = 1;
    let reminders_idx = if has_reminders { Some(2) } else { None };
    let changes_idx = if has_reminders { 3 } else { 2 };
    let activity_idx = if has_reminders { 4 } else { 3 };

    // Render each section with focus indicator and collapse state
    factory_radar::render_with_focus(
        frame,
        chunks[factory_idx],
        data,
        theme,
        focus == SidecarFocus::Factory,
        state.as_ref().and_then(|s| s.agents_state.selected()),
        supervisor_name,
        factory_collapsed,
    );
    tasks::render_with_focus(
        frame,
        chunks[tasks_idx],
        data,
        theme,
        focus == SidecarFocus::Tasks,
        state.as_ref().and_then(|s| s.tasks_state.selected()),
        agent_filter,
        tasks_collapsed,
        collapsed_epics,
        state.as_mut().map(|s| &mut *s.tasks_state),
    );

    // Render reminders panel (only when reminders exist)
    let reminders_area = if let Some(idx) = reminders_idx {
        reminders::render_with_focus(
            frame,
            chunks[idx],
            data,
            theme,
            focus == SidecarFocus::Reminders,
            reminders_collapsed,
            state.as_mut().map(|s| &mut *s.reminders_state),
        );
        chunks[idx]
    } else {
        Rect::default()
    };

    // Get collapsed_dirs from state (or empty set if no state)
    let collapsed_dirs = state
        .as_ref()
        .map(|s| s.collapsed_dirs)
        .unwrap_or(&EMPTY_SET);

    let item_types = changes::render_with_focus(
        frame,
        chunks[changes_idx],
        data,
        theme,
        focus == SidecarFocus::Changes,
        state.as_ref().and_then(|s| s.changes_state.selected()),
        changes_collapsed,
        state.as_mut().map(|s| &mut *s.changes_state),
        collapsed_dirs,
    );
    // Store item types for scroll bounds calculation
    if let Some(ref mut s) = state {
        *s.changes_item_types = item_types;
    }
    activity::render_with_focus(
        frame,
        chunks[activity_idx],
        data,
        theme,
        focus == SidecarFocus::Activity,
        state.as_ref().and_then(|s| s.activity_state.selected()),
        activity_collapsed,
    );

    // Return panel areas for click detection
    PanelAreas {
        factory: chunks[factory_idx],
        tasks: chunks[tasks_idx],
        reminders: reminders_area,
        changes: chunks[changes_idx],
        activity: chunks[activity_idx],
    }
}
