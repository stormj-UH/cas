use crate::ui::factory::app::imports::*;

impl FactoryApp {
    // ========================================================================
    // Sidecar navigation
    // ========================================================================

    /// Toggle sidecar focus (enter if None, exit if focused)
    pub fn toggle_sidecar_focus(&mut self) {
        tracing::debug!(
            "toggle_sidecar_focus called, current focus: {:?}",
            self.sidecar_focus
        );
        if self.sidecar_focus == SidecarFocus::None {
            self.sidecar_focus = SidecarFocus::Factory;
            tracing::debug!("Set sidecar_focus to Factory");
            self.init_panel_selection();
        } else {
            self.sidecar_focus = SidecarFocus::None;
            tracing::debug!("Set sidecar_focus to None");
        }
    }

    /// Move to next sidecar panel
    pub fn next_sidecar_panel(&mut self) {
        let has_reminders = !self.director_data.reminders.is_empty();
        self.sidecar_focus = self.sidecar_focus.next_with_reminders(has_reminders);
        // Initialize selection for the new panel if needed
        self.init_panel_selection();
    }

    /// Move to previous sidecar panel
    pub fn prev_sidecar_panel(&mut self) {
        let has_reminders = !self.director_data.reminders.is_empty();
        self.sidecar_focus = self.sidecar_focus.prev_with_reminders(has_reminders);
        self.init_panel_selection();
    }

    /// Focus a specific sidecar panel
    pub fn focus_sidecar_panel(&mut self, panel: SidecarFocus) {
        self.sidecar_focus = panel;
        self.init_panel_selection();
    }

    /// Get the item count for a given panel focus.
    fn item_count_for_focus(&self, focus: SidecarFocus) -> usize {
        match focus {
            SidecarFocus::None => 0,
            SidecarFocus::Factory => self.director_data.agents.len(),
            SidecarFocus::Tasks => self.task_display_item_count(),
            SidecarFocus::Reminders => self.director_data.reminders.len(),
            SidecarFocus::Changes => self.changes_item_types.len(),
            SidecarFocus::Activity => self.director_data.activity.len(),
        }
    }

    /// Initialize selection for the currently focused panel
    fn init_panel_selection(&mut self) {
        let count = self.item_count_for_focus(self.sidecar_focus);
        if let Some(p) = self.panels.get_mut(self.sidecar_focus) {
            p.init_selection(count);
        }
    }

    /// Scroll up in the current sidecar panel
    pub fn sidecar_scroll_up(&mut self) {
        if let Some(p) = self.panels.get_mut(self.sidecar_focus) {
            p.scroll_up();
        }
    }

    /// Scroll down in the current sidecar panel
    pub fn sidecar_scroll_down(&mut self) {
        let count = self.item_count_for_focus(self.sidecar_focus);
        if let Some(p) = self.panels.get_mut(self.sidecar_focus) {
            p.scroll_down(count);
        }
    }

    /// Check if sidecar is focused
    pub fn sidecar_is_focused(&self) -> bool {
        self.sidecar_focus != SidecarFocus::None
    }

    /// Register a session ID to pane name mapping
    ///
    /// This is called when a Claude session is detected to enable
    /// interaction routing to the correct pane.
    pub fn register_session(&mut self, session_id: &str, pane_name: &str) {
        tracing::info!("Registering session {} -> pane {}", session_id, pane_name);
        self.session_to_pane
            .insert(session_id.to_string(), pane_name.to_string());
    }

    /// Sync session_id → pane_name mappings from the agent store
    ///
    /// Queries registered agents and maps their session IDs to pane names.
    pub(super) fn sync_session_mappings(&mut self) {
        let agent_store = match open_agent_store(&self.cas_dir) {
            Ok(store) => store,
            Err(e) => {
                tracing::debug!("Failed to open agent store for session sync: {}", e);
                return;
            }
        };

        let agents = match agent_store.list(None) {
            Ok(agents) => agents,
            Err(e) => {
                tracing::debug!("Failed to list agents for session sync: {}", e);
                return;
            }
        };

        // Build mapping from agent session_id (agent.id) to agent.name
        // Agent IDs in CAS are typically the Claude session ID
        for agent in agents {
            // Skip agents we already have mapped
            if !self.session_to_pane.contains_key(&agent.id) {
                // Check if this agent name matches a pane we have
                if self.mux.get(&agent.name).is_some()
                    || agent.name == self.supervisor_name
                    || self.worker_names.contains(&agent.name)
                {
                    tracing::debug!(
                        "Auto-registering session {} -> pane {}",
                        agent.id,
                        agent.name
                    );
                    self.session_to_pane
                        .insert(agent.id.clone(), agent.name.clone());
                }
            }
        }
    }

    /// Handle mouse click at screen coordinates
    ///
    /// Resolves which pane was clicked and focuses it. Also handles clicks
    /// on the worker tab bar to switch worker tabs.
    pub fn handle_mouse_click(&mut self, col: u16, row: u16) {
        // Don't handle clicks while modal dialogs are open
        if self.show_task_dialog
            || self.show_changes_dialog
            || self.show_reminder_dialog
            || self.show_help
            || self.show_terminal_dialog
        {
            return;
        }

        // Check worker tab bar clicks (switches tab without focusing)
        if self.is_tabbed {
            if let Some(tab_area) = self.worker_tab_bar_area {
                if tab_area.contains((col, row).into()) {
                    let all_names = self.layout_worker_names();
                    if !all_names.is_empty() {
                        let click_x = col.saturating_sub(tab_area.x) as usize;
                        // Account for 1-char left padding before first tab
                        let mut pos: usize = 1;
                        let mut clicked_tab: Option<usize> = None;
                        for (i, name) in all_names.iter().enumerate() {
                            let number = i + 1;
                            let status_icon = if self.is_pending_worker(name) {
                                " \u{2801}" // spinner placeholder — 2 chars, same width as any frame
                            } else {
                                self.get_worker_status_icon(name)
                            };
                            // Must match renderer: format!(" {number} {name}{status_icon} ")
                            let label_width =
                                3 + number.to_string().len() + name.len() + status_icon.len();
                            if click_x >= pos && click_x < pos + label_width {
                                clicked_tab = Some(i);
                                break;
                            }
                            pos += label_width;
                            // 1-char separator between tabs
                            if i < all_names.len() - 1 {
                                pos += 1;
                            }
                        }
                        if let Some(clicked_tab) = clicked_tab {
                            self.select_worker_tab(clicked_tab);
                            // Also focus the clicked worker pane
                            if let Some(name) = self.worker_names.get(clicked_tab) {
                                let name = name.clone();
                                let _ = self.mux.focus(&name);
                                self.sidecar_focus = SidecarFocus::None;
                            }
                        }
                    }
                    return;
                }
            }
        }

        // Check sidecar area clicks
        if let Some(sidecar_area) = self.sidecar_area {
            if sidecar_area.contains((col, row).into()) {
                if self.sidecar_focus == SidecarFocus::None {
                    self.toggle_sidecar_focus();
                }
                return;
            }
        }

        // Check pane clicks (supervisor + workers)
        if let Some(pane_name) = self.pane_at_screen(col, row) {
            let _ = self.mux.focus(&pane_name);
            self.sidecar_focus = SidecarFocus::None;

            // Update selected worker tab when clicking a worker in tabbed mode
            if self.is_tabbed {
                if let Some(idx) = self.worker_names.iter().position(|n| n == &pane_name) {
                    self.selected_worker_tab = idx;
                }
            }
        }
    }

    /// Focus the next PTY pane (cycles through supervisor + worker panes only)
    pub fn focus_next_pty_pane(&mut self) {
        let pane_names = self.pty_pane_names();
        if pane_names.is_empty() {
            return;
        }

        let current = self.mux.focused_id().map(|s| s.to_string());
        let current_idx = current
            .as_ref()
            .and_then(|c| pane_names.iter().position(|n| n == c))
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % pane_names.len();
        let target = pane_names[next_idx].clone();
        let _ = self.mux.focus(&target);
        self.sidecar_focus = SidecarFocus::None;

        // Sync worker tab selection
        if let Some(idx) = self.worker_names.iter().position(|n| n == &target) {
            self.selected_worker_tab = idx;
        }
    }

    /// Focus the previous PTY pane (cycles through supervisor + worker panes only)
    pub fn focus_prev_pty_pane(&mut self) {
        let pane_names = self.pty_pane_names();
        if pane_names.is_empty() {
            return;
        }

        let current = self.mux.focused_id().map(|s| s.to_string());
        let current_idx = current
            .as_ref()
            .and_then(|c| pane_names.iter().position(|n| n == c))
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 {
            pane_names.len() - 1
        } else {
            current_idx - 1
        };
        let target = pane_names[prev_idx].clone();
        let _ = self.mux.focus(&target);
        self.sidecar_focus = SidecarFocus::None;

        // Sync worker tab selection
        if let Some(idx) = self.worker_names.iter().position(|n| n == &target) {
            self.selected_worker_tab = idx;
        }
    }

    /// Get ordered list of PTY pane names (supervisor first, then workers)
    fn pty_pane_names(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(1 + self.worker_names.len());
        names.push(self.supervisor_name.clone());
        names.extend(self.worker_names.iter().cloned());
        names
    }

    /// Handle mouse scroll up
    pub fn handle_scroll_up(&mut self) {
        if self.show_task_dialog {
            self.task_dialog_scroll = self.task_dialog_scroll.saturating_sub(1);
        } else if self.show_reminder_dialog {
            self.reminder_dialog_scroll = self.reminder_dialog_scroll.saturating_sub(1);
        } else if self.show_changes_dialog {
            self.changes_dialog_scroll = self.changes_dialog_scroll.saturating_sub(1);
        } else if self.is_mission_control()
            && self.mc_focus != crate::ui::factory::renderer::MissionControlFocus::None
        {
            self.mc_scroll_up();
        } else if self.sidecar_focus != SidecarFocus::None {
            self.sidecar_scroll_up();
        } else {
            self.scroll_focused_pane(-3);
        }
    }

    /// Handle mouse scroll down
    pub fn handle_scroll_down(&mut self) {
        if self.show_task_dialog {
            self.task_dialog_scroll =
                (self.task_dialog_scroll + 1).min(self.task_dialog_max_scroll);
        } else if self.show_reminder_dialog {
            self.reminder_dialog_scroll = self.reminder_dialog_scroll.saturating_add(1);
        } else if self.show_changes_dialog {
            let max_scroll = self.changes_dialog_diff.len().saturating_sub(10) as u16;
            self.changes_dialog_scroll = (self.changes_dialog_scroll + 1).min(max_scroll);
        } else if self.is_mission_control()
            && self.mc_focus != crate::ui::factory::renderer::MissionControlFocus::None
        {
            self.mc_scroll_down();
        } else if self.sidecar_focus != SidecarFocus::None {
            self.sidecar_scroll_down();
        } else {
            self.scroll_focused_pane(3);
        }
    }

    /// Convert screen coordinates to the pane at that position.
    ///
    /// Returns the pane name if the coordinates are inside a pane.
    pub fn pane_at_screen(&self, x: u16, y: u16) -> Option<String> {
        let point = (x, y);

        // Check supervisor area
        if let Some(sup_area) = self.supervisor_area {
            if sup_area.contains(point.into()) {
                return Some(self.supervisor_name.clone());
            }
        }

        // Check worker areas
        if self.is_tabbed {
            if let Some(content_area) = self.worker_content_area {
                if content_area.contains(point.into()) {
                    return self.worker_names.get(self.selected_worker_tab).cloned();
                }
            }
        } else {
            for (i, worker_area) in self.worker_areas.iter().enumerate() {
                if worker_area.contains(point.into()) {
                    return self.worker_names.get(i).cloned();
                }
            }
        }

        None
    }

    /// Scroll the supervisor pane by delta lines
    pub fn scroll_supervisor(&mut self, delta: i32) {
        if let Err(e) = self.mux.scroll_pane(&self.supervisor_name, delta) {
            tracing::warn!("Failed to scroll supervisor pane: {}", e);
        }
    }

    /// Scroll the focused pane by delta lines
    pub fn scroll_focused_pane(&mut self, delta: i32) {
        if let Err(e) = self.mux.scroll_focused(delta) {
            tracing::warn!("Failed to scroll focused pane: {}", e);
        }
    }

    /// Return the bytes to forward to the focused PTY when a mouse-wheel scroll
    /// event arrives while the inner process is in alt-screen mode.
    ///
    /// Returns `Some(bytes)` only when **all** of the following are true:
    /// 1. No dialog (task / reminder / changes) is open — those handle scroll
    ///    themselves via `handle_scroll_up/down`.
    /// 2. The sidecar is not focused.
    /// 3. Mission Control is not active with a focused panel.
    /// 4. The focused pane's inner process is in alt-screen mode.
    ///
    /// When `Some` is returned the caller should forward the bytes to the
    /// focused PTY (e.g., `mux.send_input(bytes).await`) instead of calling
    /// `handle_scroll_up/down`. The payload is 3× arrow-up or arrow-down
    /// (matching the `scroll_focused_pane(±3)` line granularity).
    ///
    /// Returns `None` in all other cases — the normal `handle_scroll_up/down`
    /// path should be used.
    pub fn alt_screen_scroll_input(&self, scroll_up: bool) -> Option<&'static [u8]> {
        // Conditions that would prevent reaching scroll_focused_pane normally
        if self.show_task_dialog
            || self.show_reminder_dialog
            || self.show_changes_dialog
            || self.sidecar_focus != SidecarFocus::None
            || self.is_mission_control()
        {
            return None;
        }
        if self.mux.focused_is_in_alt_screen() {
            // 3 arrows = same line granularity as scroll_focused_pane(±3)
            if scroll_up {
                Some(b"\x1b[A\x1b[A\x1b[A")
            } else {
                Some(b"\x1b[B\x1b[B\x1b[B")
            }
        } else {
            None
        }
    }

    /// Scroll the supervisor pane to bottom (most recent content)
    pub fn scroll_supervisor_to_bottom(&mut self) {
        if let Err(e) = self.mux.scroll_pane_to_bottom(&self.supervisor_name) {
            tracing::warn!("Failed to scroll supervisor to bottom: {}", e);
        }
    }

    /// Handle Enter key - open detail dialog for selected item
    pub fn handle_enter(&mut self) {
        if self.view_mode == ViewMode::Overview {
            match self.sidecar_focus {
                SidecarFocus::Factory => {
                    if let Some(idx) = self.panels.factory.list_state.selected() {
                        if let Some(agent) = self.director_data.agents.get(idx) {
                            let _ = self.mux.focus(&agent.name);
                            self.sidecar_focus = SidecarFocus::None;
                        }
                    }
                }
                SidecarFocus::Tasks => {
                    // Open task detail dialog
                    self.open_task_dialog();
                }
                SidecarFocus::Reminders => {
                    // Open reminder detail dialog
                    self.open_reminder_dialog();
                }
                SidecarFocus::Changes => {
                    // Open file changes dialog for selected change
                    self.open_changes_dialog();
                }
                SidecarFocus::Activity => {
                    self.detail_scroll = 0;
                    self.view_mode = ViewMode::ActivityLog;
                }
                _ => {}
            }
        }
    }

    /// Open the task detail dialog for the selected task
    pub fn open_task_dialog(&mut self) {
        if let Some(task_id) = self.get_selected_task_id() {
            self.task_dialog_id = Some(task_id);
            self.task_dialog_scroll = 0;
            self.show_task_dialog = true;
        }
    }

    /// Close the task detail dialog
    pub fn close_task_dialog(&mut self) {
        self.show_task_dialog = false;
        self.task_dialog_id = None;
        self.task_dialog_scroll = 0;
    }

    /// Open the reminder detail dialog for the selected reminder
    pub fn open_reminder_dialog(&mut self) {
        if let Some(idx) = self.panels.reminders.list_state.selected() {
            if idx < self.director_data.reminders.len() {
                self.reminder_dialog_idx = Some(idx);
                self.reminder_dialog_scroll = 0;
                self.show_reminder_dialog = true;
            }
        }
    }

    /// Close the reminder detail dialog
    pub fn close_reminder_dialog(&mut self) {
        self.show_reminder_dialog = false;
        self.reminder_dialog_idx = None;
        self.reminder_dialog_scroll = 0;
    }

    /// Handle Escape key - return to overview or unfocus sidecar
    pub fn handle_escape(&mut self) -> bool {
        // Close task dialog if open
        if self.show_task_dialog {
            self.close_task_dialog();
            return true;
        }

        // Close reminder dialog if open
        if self.show_reminder_dialog {
            self.close_reminder_dialog();
            return true;
        }

        // Close changes dialog if open
        if self.show_changes_dialog {
            self.close_changes_dialog();
            return true;
        }

        match &self.view_mode {
            ViewMode::TaskDetail(_) | ViewMode::ActivityLog => {
                self.view_mode = ViewMode::Overview;
                true
            }
            ViewMode::Overview | ViewMode::FileDiff(_, _) => {
                if self.sidecar_focus != SidecarFocus::None {
                    self.sidecar_focus = SidecarFocus::None;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Compute the total number of display items in the task panel.
    ///
    /// Must match the item count produced by `tasks::render_with_focus()`,
    /// including epic headers, subtasks (when not collapsed), separators, and standalone tasks.
    fn task_display_item_count(&self) -> usize {
        let (epic_groups, standalone) = self.director_data.tasks_by_epic();
        let agent_filter = self.agent_filter.as_deref();
        let mut count = 0;

        for group in &epic_groups {
            let visible_subtasks: usize = group
                .subtasks
                .iter()
                .filter(|t| match agent_filter {
                    None => true,
                    Some(filter) => t.assignee.as_deref() == Some(filter),
                })
                .count();

            if agent_filter.is_some() && visible_subtasks == 0 {
                continue;
            }

            count += 1; // epic header row

            if !self.collapsed_epics.contains(&group.epic.id) {
                count += visible_subtasks;
            }
        }

        let filtered_standalone_count = standalone
            .iter()
            .filter(|t| match agent_filter {
                None => true,
                Some(filter) => t.assignee.as_deref() == Some(filter),
            })
            .count();

        if count > 0 && filtered_standalone_count > 0 {
            count += 1; // separator row
        }
        count += filtered_standalone_count;
        count
    }

    /// Get the ID of the selected task (if any).
    ///
    /// Walks through display items (epic headers, subtasks, separators, standalone)
    /// to correctly map the selected display index to a task ID.
    fn get_selected_task_id(&self) -> Option<String> {
        let selected = self.panels.tasks.list_state.selected()?;
        let (epic_groups, standalone) = self.director_data.tasks_by_epic();
        let agent_filter = self.agent_filter.as_deref();
        let mut idx = 0;

        for group in &epic_groups {
            let filtered_subtasks: Vec<_> = group
                .subtasks
                .iter()
                .filter(|t| match agent_filter {
                    None => true,
                    Some(filter) => t.assignee.as_deref() == Some(filter),
                })
                .collect();

            if agent_filter.is_some() && filtered_subtasks.is_empty() {
                continue;
            }

            if idx == selected {
                return Some(group.epic.id.clone());
            }
            idx += 1;

            if !self.collapsed_epics.contains(&group.epic.id) {
                for task in &filtered_subtasks {
                    if idx == selected {
                        return Some(task.id.clone());
                    }
                    idx += 1;
                }
            }
        }

        let filtered_standalone: Vec<_> = standalone
            .iter()
            .filter(|t| match agent_filter {
                None => true,
                Some(filter) => t.assignee.as_deref() == Some(filter),
            })
            .collect();

        if idx > 0 && !filtered_standalone.is_empty() {
            if idx == selected {
                return None; // separator row
            }
            idx += 1;
        }

        for task in &filtered_standalone {
            if idx == selected {
                return Some(task.id.clone());
            }
            idx += 1;
        }

        None
    }

    /// Get the selected task (if any).
    pub fn get_selected_task(&self) -> Option<&crate::ui::factory::director::TaskSummary> {
        let selected = self.panels.tasks.list_state.selected()?;
        let (epic_groups, standalone) = self.director_data.tasks_by_epic();
        let agent_filter = self.agent_filter.as_deref();
        let mut idx = 0;

        for group in &epic_groups {
            let filtered_subtask_indices: Vec<usize> = group
                .subtasks
                .iter()
                .enumerate()
                .filter(|(_, t)| match agent_filter {
                    None => true,
                    Some(filter) => t.assignee.as_deref() == Some(filter),
                })
                .map(|(i, _)| i)
                .collect();

            if agent_filter.is_some() && filtered_subtask_indices.is_empty() {
                continue;
            }

            if idx == selected {
                return None; // epic header, not a task
            }
            idx += 1;

            if !self.collapsed_epics.contains(&group.epic.id) {
                for &task_idx in &filtered_subtask_indices {
                    if idx == selected {
                        let task = &group.subtasks[task_idx];
                        return self
                            .director_data
                            .in_progress_tasks
                            .iter()
                            .chain(self.director_data.ready_tasks.iter())
                            .find(|t| t.id == task.id);
                    }
                    idx += 1;
                }
            }
        }

        let filtered_standalone: Vec<_> = standalone
            .iter()
            .filter(|t| match agent_filter {
                None => true,
                Some(filter) => t.assignee.as_deref() == Some(filter),
            })
            .collect();

        if idx > 0 && !filtered_standalone.is_empty() {
            if idx == selected {
                return None; // separator
            }
            idx += 1;
        }

        for task in &filtered_standalone {
            if idx == selected {
                return self
                    .director_data
                    .in_progress_tasks
                    .iter()
                    .chain(self.director_data.ready_tasks.iter())
                    .find(|t| t.id == task.id);
            }
            idx += 1;
        }

        None
    }

    /// Scroll detail view up
    pub fn detail_scroll_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
    }

    /// Scroll detail view down
    pub fn detail_scroll_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(1);
    }

    // ========================================================================
    // Mission Control navigation
    // ========================================================================

    /// Check if we are in Mission Control mode.
    pub fn is_mission_control(&self) -> bool {
        self.factory_view_mode == crate::ui::factory::renderer::FactoryViewMode::MissionControl
    }

    /// Cycle Mission Control focus to the next panel.
    pub fn mc_focus_next(&mut self) {
        self.mc_focus = self.mc_focus.next();
        self.mc_init_panel_selection();
    }

    /// Cycle Mission Control focus to the previous panel.
    pub fn mc_focus_prev(&mut self) {
        self.mc_focus = self.mc_focus.prev();
        self.mc_init_panel_selection();
    }

    /// Jump MC focus to a specific panel.
    pub fn mc_focus_panel(&mut self, panel: crate::ui::factory::renderer::MissionControlFocus) {
        self.mc_focus = panel;
        self.mc_init_panel_selection();
    }

    /// Initialize selection for the MC-focused panel.
    fn mc_init_panel_selection(&mut self) {
        let focus = self.mc_focus.to_sidecar_focus();
        let count = self.item_count_for_focus(focus);
        if let Some(p) = self.panels.get_mut(focus) {
            p.init_selection(count);
        }
    }

    /// Scroll up in the MC-focused panel.
    pub fn mc_scroll_up(&mut self) {
        if let Some(p) = self.panels.get_mut(self.mc_focus.to_sidecar_focus()) {
            p.scroll_up();
        }
    }

    /// Scroll down in the MC-focused panel.
    pub fn mc_scroll_down(&mut self) {
        let focus = self.mc_focus.to_sidecar_focus();
        let count = self.item_count_for_focus(focus);
        if let Some(p) = self.panels.get_mut(focus) {
            p.scroll_down(count);
        }
    }

    /// Handle Enter in Mission Control view.
    pub fn mc_handle_enter(&mut self) {
        use crate::ui::factory::renderer::MissionControlFocus;
        match self.mc_focus {
            MissionControlFocus::Workers => {
                // Focus the selected worker's PTY and switch to Panes view
                if let Some(idx) = self.panels.factory.list_state.selected() {
                    if let Some(agent) = self.director_data.agents.get(idx) {
                        let _ = self.mux.focus(&agent.name);
                        self.factory_view_mode =
                            crate::ui::factory::renderer::FactoryViewMode::Panes;
                    }
                }
            }
            MissionControlFocus::Tasks => {
                self.open_task_dialog();
            }
            MissionControlFocus::Changes => {
                self.open_changes_dialog();
            }
            MissionControlFocus::Activity | MissionControlFocus::None => {}
        }
    }

    /// Handle Escape in Mission Control view. Returns true if something was closed.
    pub fn mc_handle_escape(&mut self) -> bool {
        // Close any open dialog first
        if self.show_task_dialog {
            self.close_task_dialog();
            return true;
        }
        if self.show_reminder_dialog {
            self.close_reminder_dialog();
            return true;
        }
        if self.show_changes_dialog {
            self.close_changes_dialog();
            return true;
        }
        // If a panel is focused, unfocus it
        if self.mc_focus != crate::ui::factory::renderer::MissionControlFocus::None {
            self.mc_focus = crate::ui::factory::renderer::MissionControlFocus::None;
            return true;
        }
        // Otherwise switch back to Panes view
        self.factory_view_mode = crate::ui::factory::renderer::FactoryViewMode::Panes;
        true
    }

    /// Enter inject mode from Mission Control.
    /// Targets the selected worker (if Workers panel focused), otherwise supervisor.
    pub fn mc_start_inject(&mut self) {
        use crate::ui::factory::renderer::MissionControlFocus;
        let target = if self.mc_focus == MissionControlFocus::Workers {
            // Use selected worker
            self.panels
                .factory
                .list_state
                .selected()
                .and_then(|idx| self.director_data.agents.get(idx))
                .map(|a| a.name.clone())
        } else {
            None
        };
        let target_name = target.unwrap_or_else(|| self.supervisor_name.clone());
        self.inject_target = Some(target_name);
        self.inject_buffer.clear();
        self.input_mode = InputMode::Inject;
    }

    /// Toggle epic collapse from Mission Control Tasks panel.
    pub fn mc_toggle_collapse(&mut self) {
        use crate::ui::factory::renderer::MissionControlFocus;
        match self.mc_focus {
            MissionControlFocus::Tasks => {
                // Reuse existing epic collapse logic (it checks sidecar_focus internally,
                // so we temporarily set it)
                let saved = self.sidecar_focus;
                self.sidecar_focus = SidecarFocus::Tasks;
                self.toggle_epic_collapse();
                self.sidecar_focus = saved;
            }
            MissionControlFocus::Changes => {
                let saved = self.sidecar_focus;
                self.sidecar_focus = SidecarFocus::Changes;
                self.toggle_selected_dir_collapse_mc();
                self.sidecar_focus = saved;
            }
            _ => {}
        }
    }

    /// Toggle collapse for selected directory in changes panel (MC variant).
    fn toggle_selected_dir_collapse_mc(&mut self) {
        let Some(selected_idx) = self.panels.changes.list_state.selected() else {
            return;
        };
        if let Some(crate::ui::widgets::TreeItemType::Directory(dir_path)) =
            self.changes_item_types.get(selected_idx)
        {
            if self.collapsed_dirs.contains(dir_path) {
                self.collapsed_dirs.remove(dir_path);
            } else {
                self.collapsed_dirs.insert(dir_path.clone());
            }
        }
    }
}
