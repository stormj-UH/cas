//! Pane abstraction using ghostty_vt for terminal emulation
//!
//! A pane combines:
//! - A PTY process (optional - director pane is native)
//! - A ghostty_vt Terminal for state management
//! - Metadata (agent name, role, etc.)

mod snapshot;
mod style;
mod tests;

use crate::error::{Error, Result};
use crate::harness::SupervisorCli;
use crate::pane::style::{cell_style_to_ratatui, debug_log_enabled};
use crate::pty::{Pty, PtyConfig, PtyEvent, TeamsSpawnConfig};
pub use cas_factory_protocol::TerminalSnapshot;
use ghostty_vt::{CellStyle, Rgb, Terminal};
use ratatui::text::{Line, Span};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use cas_recording::{RecordingWriter, WriterConfig};

/// Unique identifier for a pane
pub type PaneId = String;

/// The kind of pane
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneKind {
    /// Worker agent (Claude/Codex CLI)
    Worker,
    /// Supervisor agent (Claude/Codex CLI)
    Supervisor,
    /// Director (native TUI, no PTY)
    Director,
    /// Generic shell
    Shell,
}

impl PaneKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Supervisor => "supervisor",
            Self::Director => "director",
            Self::Shell => "shell",
        }
    }
}

/// Backend for a pane — either a PTY (Claude/Codex interactive) or
/// none (director pane).
pub enum PaneBackend {
    /// No backend (director pane — rendered natively)
    None,
    /// PTY-based interactive terminal (Claude, Codex)
    Pty(Pty),
}

/// A pane in the multiplexer
pub struct Pane {
    /// Unique identifier (usually agent name)
    id: PaneId,
    /// What kind of pane
    kind: PaneKind,
    /// The ghostty_vt terminal (handles escape sequences, cursor, colors)
    pub(crate) terminal: Terminal,
    /// Process backend
    backend: PaneBackend,
    /// Whether this pane has focus
    focused: bool,
    /// Title for display
    title: String,
    /// Color for the pane border (hex)
    color: Option<String>,
    /// Whether the process has exited
    exited: bool,
    /// Exit code if exited
    exit_code: Option<i32>,
    /// Terminal dimensions
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    /// Optional recording writer for session capture
    recorder: Option<Arc<Mutex<RecordingWriter>>>,
    /// Whether to force all rows dirty on next take (for new client sync)
    force_all_dirty: bool,
    /// Last known total scrollback lines (for scroll detection)
    pub(crate) last_total_scrollback: u32,
    /// Sequence counter for incremental updates (pane-scoped)
    pub(crate) seq_counter: u64,
    /// Whether the user has scrolled up from the bottom
    user_scrolled: bool,
    /// Number of new output lines received while user was scrolled up
    new_lines_below: u32,
    /// Reusable scratch buffer for drain_output (avoids 65KB alloc per poll)
    drain_buf: Vec<u8>,
    /// Total bytes of output received from the process (for readiness detection)
    total_bytes_received: u64,
    /// When this pane was created (for startup grace period)
    created_at: std::time::Instant,
}

impl Pane {
    /// Create a new pane with a specific backend.
    fn new_with_backend(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: PaneKind,
        backend: PaneBackend,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let id = id.into();
        let mut terminal = Terminal::new(rows, cols).map_err(|e| Error::terminal(e.to_string()))?;
        terminal.set_default_colors(Rgb { r: 0, g: 0, b: 0 }, Rgb { r: 0, g: 0, b: 0 });
        let info = terminal.scrollback_info();
        Ok(Self {
            title: title.into(),
            id,
            kind,
            terminal,
            backend,
            focused: false,
            color: None,
            exited: false,
            exit_code: None,
            rows,
            cols,
            recorder: None,
            force_all_dirty: true,
            last_total_scrollback: info.total_scrollback,
            seq_counter: 0,
            user_scrolled: false,
            new_lines_below: 0,
            drain_buf: Vec::with_capacity(65536),
            total_bytes_received: 0,
            created_at: std::time::Instant::now(),
        })
    }

    /// Create a new pane with a PTY
    pub fn with_pty(
        id: impl Into<String>,
        kind: PaneKind,
        pty: Pty,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let id_str: String = id.into();
        Self::new_with_backend(
            id_str.clone(),
            id_str,
            kind,
            PaneBackend::Pty(pty),
            rows,
            cols,
        )
    }

    /// Create a director pane (no PTY)
    pub fn director(id: impl Into<String>, rows: u16, cols: u16) -> Result<Self> {
        let id_str: String = id.into();
        Self::new_with_backend(
            id_str,
            "Director",
            PaneKind::Director,
            PaneBackend::None,
            rows,
            cols,
        )
    }

    /// Create a shell pane running the user's default shell (or a specific command).
    pub fn shell(
        name: &str,
        cwd: PathBuf,
        shell_command: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let shell = shell_command
            .map(|s| s.to_string())
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()));

        let config = PtyConfig {
            command: shell,
            args: vec![],
            cwd: Some(cwd),
            env: vec![],
            rows,
            cols,
        };
        let pty = Pty::spawn(name, config)?;
        Self::with_pty(name, PaneKind::Shell, pty, rows, cols)
    }

    /// Build the `PtyConfig` that `worker()` would spawn, without actually
    /// spawning a process. Used by `Mux::factory_pane_configs` and tests.
    #[allow(clippy::too_many_arguments)]
    pub fn build_worker_config(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: &str,
        cli: SupervisorCli,
        model: Option<&str>,
        effort: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
    ) -> PtyConfig {
        match cli {
            SupervisorCli::Claude => PtyConfig::claude(
                name,
                "worker",
                cwd,
                cas_root,
                Some(supervisor_name),
                None,
                model,
                effort,
                teams,
            ),
            SupervisorCli::Codex => PtyConfig::codex(
                name,
                "worker",
                cwd,
                cas_root,
                Some(supervisor_name),
                None,
                model,
                effort,
                teams,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn worker(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: &str,
        cli: SupervisorCli,
        model: Option<&str>,
        effort: Option<&str>,
        rows: u16,
        cols: u16,
        teams: Option<&TeamsSpawnConfig>,
    ) -> Result<Self> {
        let config = Self::build_worker_config(
            name, cwd, cas_root, supervisor_name, cli, model, effort, teams,
        );
        let pty = Pty::spawn(name, config)?;
        Self::with_pty(name, PaneKind::Worker, pty, rows, cols)
    }

    /// Build the `PtyConfig` that `supervisor()` would spawn, without actually
    /// spawning a process. Used by `Mux::factory_pane_configs` and tests.
    #[allow(clippy::too_many_arguments)]
    pub fn build_supervisor_config(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        cli: SupervisorCli,
        worker_cli: SupervisorCli,
        worker_names: &[String],
        model: Option<&str>,
        effort: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
    ) -> PtyConfig {
        let worker_cli_str = worker_cli.as_str();
        let worker_names_csv = if worker_names.is_empty() {
            None
        } else {
            Some(worker_names.join(","))
        };
        let mut config = match cli {
            SupervisorCli::Claude => PtyConfig::claude(
                name,
                "supervisor",
                cwd,
                cas_root,
                None,
                Some(worker_cli_str),
                model,
                effort,
                teams,
            ),
            SupervisorCli::Codex => PtyConfig::codex(
                name,
                "supervisor",
                cwd,
                cas_root,
                None,
                Some(worker_cli_str),
                model,
                effort,
                teams,
            ),
        };
        Self::push_supervisor_env(&mut config.env, cli, &worker_names_csv);
        config
    }

    #[allow(clippy::too_many_arguments)]
    pub fn supervisor(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        rows: u16,
        cols: u16,
        cli: SupervisorCli,
        worker_cli: SupervisorCli,
        worker_names: &[String],
        model: Option<&str>,
        effort: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
    ) -> Result<Self> {
        let config = Self::build_supervisor_config(
            name, cwd, cas_root, cli, worker_cli, worker_names, model, effort, teams,
        );
        let pty = Pty::spawn(name, config)?;
        Self::with_pty(name, PaneKind::Supervisor, pty, rows, cols)
    }

    fn push_supervisor_env(
        env: &mut Vec<(String, String)>,
        cli: SupervisorCli,
        worker_names_csv: &Option<String>,
    ) {
        env.push((
            "CAS_FACTORY_SUPERVISOR_CLI".to_string(),
            cli.as_str().to_string(),
        ));
        if let Some(csv) = worker_names_csv {
            env.push(("CAS_FACTORY_WORKER_NAMES".to_string(), csv.clone()));
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn kind(&self) -> &PaneKind {
        &self.kind
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }

    pub fn set_color(&mut self, color: impl Into<String>) {
        self.color = Some(color.into());
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn mark_all_dirty(&mut self) {
        self.force_all_dirty = true;
    }

    pub(crate) fn take_force_all_dirty(&mut self) -> bool {
        std::mem::take(&mut self.force_all_dirty)
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        self.terminal.cursor_position()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        if debug_log_enabled() {
            tracing::debug!(
                "Pane {}: resize from {}x{} to {}x{}",
                self.id,
                self.rows,
                self.cols,
                rows,
                cols
            );
        }
        self.terminal.resize(rows, cols).map_err(|e| {
            tracing::warn!("Pane {}: terminal.resize failed: {}", self.id, e);
            Error::terminal(e.to_string())
        })?;
        self.rows = rows;
        self.cols = cols;
        match &self.backend {
            PaneBackend::Pty(pty) => pty.resize(rows, cols)?,
            PaneBackend::None => {}
        }
        Ok(())
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<()> {
        if self.user_scrolled {
            // Save scroll position before feeding new data
            let before = self.terminal.scrollback_info();
            let old_total = before.total_scrollback;
            let old_offset = before.viewport_offset;

            self.terminal
                .feed(data)
                .map_err(|e| Error::terminal(e.to_string()))?;

            let after = self.terminal.scrollback_info();
            let new_lines = after.total_scrollback.saturating_sub(old_total);
            if new_lines > 0 {
                self.new_lines_below = self.new_lines_below.saturating_add(new_lines);
            }

            // Preserve viewport: the user should see the same content as before feed.
            // Target offset = old_offset + new_lines (same absolute position, measured
            // from the new bottom which is now further away by new_lines).
            // The terminal may or may not auto-scroll after feed — check the actual
            // offset and only adjust the delta needed.
            let target_offset = old_offset.saturating_add(new_lines);
            let current_offset = after.viewport_offset;
            if current_offset != target_offset {
                // Positive delta = scroll down (toward bottom), negative = scroll up
                let delta = current_offset as i32 - target_offset as i32;
                let _ = self.terminal.scroll(delta);
            }

            Ok(())
        } else {
            self.terminal
                .feed(data)
                .map_err(|e| Error::terminal(e.to_string()))
        }
    }

    /// Strip literal cursor-position report echoes such as `^[[1;1R`.
    ///
    /// Some agent CLIs emit this as plain text when probing terminal support,
    /// which creates visual noise in pane output.
    fn strip_literal_cursor_reports(data: &[u8]) -> Cow<'_, [u8]> {
        let mut out: Option<Vec<u8>> = None;
        let mut i = 0usize;
        let mut last_emit = 0usize;

        while i < data.len() {
            if let Some(len) = Self::literal_cursor_report_len(&data[i..]) {
                let out_buf = out.get_or_insert_with(|| Vec::with_capacity(data.len()));
                out_buf.extend_from_slice(&data[last_emit..i]);
                i += len;
                last_emit = i;
                continue;
            }
            i += 1;
        }

        if let Some(mut out_buf) = out {
            out_buf.extend_from_slice(&data[last_emit..]);
            Cow::Owned(out_buf)
        } else {
            Cow::Borrowed(data)
        }
    }

    fn literal_cursor_report_len(data: &[u8]) -> Option<usize> {
        // Matches: ^[[<row>;<col>R
        if data.len() < 7 || data[0] != b'^' || data[1] != b'[' || data[2] != b'[' {
            return None;
        }

        let mut idx = 3;
        let row_start = idx;
        while idx < data.len() && data[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == row_start || idx >= data.len() || data[idx] != b';' {
            return None;
        }

        idx += 1;
        let col_start = idx;
        while idx < data.len() && data[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == col_start || idx >= data.len() || data[idx] != b'R' {
            return None;
        }

        Some(idx + 1)
    }

    pub fn dump_viewport(&self) -> Result<String> {
        self.terminal
            .dump_viewport()
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn dump_row(&self, row: u16) -> Result<String> {
        self.terminal
            .dump_viewport_row(row)
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn row_styles(&self, row: u16) -> Result<Vec<CellStyle>> {
        self.terminal
            .row_cell_styles(row)
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn row_as_line(&self, row: u16) -> Result<Line<'static>> {
        let text = self.dump_row(row)?;
        // Use style runs (pre-grouped by the VT) instead of per-cell styles
        // to avoid a separate O(cols) traversal + per-cell comparison.
        let runs = self.terminal.row_style_runs(row).map_err(|e| Error::terminal(e.to_string()))?;

        if runs.is_empty() {
            return Ok(Line::from(vec![Span::raw(text)]));
        }

        let chars: Vec<char> = text.chars().collect();
        let mut spans = Vec::with_capacity(runs.len());

        for run in &runs {
            let start = run.start_col as usize;
            let end = (run.end_col as usize).min(chars.len());
            if start >= chars.len() {
                break;
            }
            let span_text: String = chars[start..end].iter().collect();
            let style = cell_style_to_ratatui(&run.style);
            spans.push(Span::styled(span_text, style));
        }

        if spans.is_empty() && !text.is_empty() {
            spans.push(Span::raw(text));
        }

        Ok(Line::from(spans))
    }

    pub fn viewport_as_lines(&self) -> Result<Vec<Line<'static>>> {
        let mut lines = Vec::with_capacity(self.rows as usize);
        for row in 0..self.rows {
            lines.push(self.row_as_line(row)?);
        }
        Ok(lines)
    }

    pub fn poll(&mut self) -> Option<PtyEvent> {
        let event = match &mut self.backend {
            PaneBackend::Pty(pty) => pty.try_recv(),
            PaneBackend::None => None,
        }?;

        match &event {
            PtyEvent::Output(data) => {
                let feed_data = Self::strip_literal_cursor_reports(data);
                if let Err(e) = self.feed(feed_data.as_ref()) {
                    tracing::warn!("Failed to feed data to terminal: {}", e);
                }
            }
            PtyEvent::Exited(code) => {
                self.exited = true;
                self.exit_code = *code;
            }
            PtyEvent::Error(_) => {
                self.exited = true;
            }
        }
        Some(event)
    }

    pub fn drain_output(&mut self) -> (Vec<u8>, Vec<PtyEvent>) {
        let mut other_events = Vec::new();
        self.drain_buf.clear();

        let try_recv = |backend: &mut PaneBackend| -> Option<PtyEvent> {
            match backend {
                PaneBackend::Pty(pty) => pty.try_recv(),
                PaneBackend::None => None,
            }
        };

        while let Some(event) = try_recv(&mut self.backend) {
            match event {
                PtyEvent::Output(data) => {
                    self.drain_buf.extend_from_slice(&data);
                }
                PtyEvent::Exited(code) => {
                    self.exited = true;
                    self.exit_code = code;
                    other_events.push(PtyEvent::Exited(code));
                }
                PtyEvent::Error(e) => {
                    self.exited = true;
                    other_events.push(PtyEvent::Error(e));
                }
            }
        }

        // Take the buffer out to avoid borrow conflict with self.feed()
        let coalesced = std::mem::take(&mut self.drain_buf);

        if !coalesced.is_empty() {
            self.total_bytes_received += coalesced.len() as u64;
            let feed_data = Self::strip_literal_cursor_reports(&coalesced);
            if let Err(e) = self.feed(feed_data.as_ref()) {
                tracing::warn!(
                    "Failed to feed {} bytes to terminal: {}",
                    feed_data.len(),
                    e
                );
            }
        }

        // Return the coalesced data directly — no clone needed since take()
        // already moved ownership out. drain_buf capacity is donated to the
        // caller but re-grows cheaply on the next cycle.
        (coalesced, other_events)
    }

    pub async fn write(&self, data: &[u8]) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                pty.write(data).await?;
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    pub async fn send_line(&self, line: &str) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                pty.send_line(line).await?;
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    /// Whether this pane is ready to accept prompt injection.
    /// Claude Code flushes the PTY input buffer during startup, so text
    /// written before readline initialization is silently lost. We require
    /// both output (Claude has booted) AND a 5-second grace period.
    pub fn ready_for_injection(&self) -> bool {
        self.total_bytes_received > 0
            && self.created_at.elapsed() >= std::time::Duration::from_secs(5)
    }

    pub async fn inject_prompt(&self, prompt: &str) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                let text = prompt.trim();
                pty.write(text.as_bytes()).await?;
                // Send carriage return after a settle delay in a background task
                // so we don't block the daemon event loop for 150-500ms.
                let writer = pty.writer_handle();
                let settle_ms = if pty.is_codex() { 500 } else { 150 };
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(settle_ms)).await;
                    let mut guard = writer.lock().await;
                    let _ = guard.write_all(b"\r");
                    let _ = guard.flush();
                });
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    pub async fn interrupt(&self) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                pty.interrupt().await?;
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    pub fn scroll(&mut self, delta: i32) -> Result<()> {
        let info_before = self.terminal.scrollback_info();
        if debug_log_enabled() {
            tracing::debug!(
                "Pane {}: scroll delta={}, before: offset={}, total={}",
                self.id,
                delta,
                info_before.viewport_offset,
                info_before.total_scrollback
            );
        }
        let result = self
            .terminal
            .scroll(delta)
            .map_err(|e| Error::terminal(e.to_string()));
        let info_after = self.terminal.scrollback_info();

        // Track whether user has scrolled away from bottom
        if info_after.viewport_offset > 0 {
            self.user_scrolled = true;
        } else {
            self.user_scrolled = false;
            self.new_lines_below = 0;
        }

        if debug_log_enabled() {
            tracing::debug!(
                "Pane {}: scroll complete, after: offset={}, total={}",
                self.id,
                info_after.viewport_offset,
                info_after.total_scrollback
            );
        }
        result
    }

    pub fn scroll_to_top(&mut self) -> Result<()> {
        self.terminal
            .scroll_to_top()
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn scroll_to_bottom(&mut self) -> Result<()> {
        self.user_scrolled = false;
        self.new_lines_below = 0;
        self.terminal
            .scroll_to_bottom()
            .map_err(|e| Error::terminal(e.to_string()))
    }

    /// Whether the user has scrolled up from the bottom
    pub fn is_user_scrolled(&self) -> bool {
        self.user_scrolled
    }

    /// Number of new output lines received while user was scrolled up
    pub fn new_lines_below(&self) -> u32 {
        self.new_lines_below
    }

    pub fn kill(&mut self) {
        match &mut self.backend {
            PaneBackend::Pty(pty) => pty.kill(),
            PaneBackend::None => {}
        }
    }

    pub async fn start_recording(
        &mut self,
        session_id: impl Into<String>,
        config: WriterConfig,
    ) -> Result<()> {
        if self.recorder.is_some() {
            return Err(Error::recording("Recording already in progress"));
        }

        let writer = RecordingWriter::new(
            self.cols,
            self.rows,
            self.id.clone(),
            session_id.into(),
            self.kind.as_str(),
            config,
        )
        .await
        .map_err(|e| Error::recording(e.to_string()))?;

        self.recorder = Some(Arc::new(Mutex::new(writer)));

        self.generate_keyframe().await?;

        tracing::info!("Started recording for pane {}", self.id);
        Ok(())
    }

    pub async fn stop_recording(&mut self) -> Result<Option<PathBuf>> {
        if let Some(recorder) = self.recorder.take() {
            let writer = match Arc::try_unwrap(recorder) {
                Ok(mutex) => mutex.into_inner(),
                Err(_) => return Err(Error::recording("Recording still in use")),
            };
            let path = writer.file_path().clone();
            writer
                .close()
                .await
                .map_err(|e| Error::recording(e.to_string()))?;
            tracing::info!(
                "Stopped recording for pane {}, saved to {:?}",
                self.id,
                path
            );
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    async fn generate_keyframe(&mut self) -> Result<()> {
        if let Some(ref recorder) = self.recorder {
            let mut lines = Vec::new();
            for row in 0..self.rows {
                let text = self
                    .terminal
                    .dump_screen_row(row as u32)
                    .unwrap_or_default();
                lines.push(text);
            }
            let content = lines.join("\n").into_bytes();

            let mut writer = recorder.lock().await;
            writer
                .write_keyframe(content)
                .await
                .map_err(|e| Error::recording(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn record_output(&mut self, data: &[u8]) -> Result<()> {
        if let Some(ref recorder) = self.recorder {
            let writer = recorder.lock().await;
            writer
                .write_output(data)
                .await
                .map_err(|e| Error::recording(e.to_string()))?;
        }
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }
}
